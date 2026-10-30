//! 平台实现 → 会话 trait 的适配器（§3.9「通过 trait 注入，便于测试」）。
//!
//! ## D3D11 设备契约（§3.10 / §6「capture→encode 同一设备零拷贝」）
//!
//! D3D11 **不允许跨设备拷贝**：抓屏纹理必须和编码器的硬件帧上下文
//! 建在**同一个 `ID3D11Device`** 上，否则 `CopyResource` 无从谈起。
//!
//! 这条不变量在本 crate 的成立方式是：
//!
//! - 抓屏层把 D3D11 纹理（`beam_capture::Texture`）原样交给编码层，
//!   本 crate **不复制、不重建**纹理；
//! - 编码层建原生上下文时用 `ID3D11Texture2D::GetDevice()` 取**这张纹理自己的**
//!   设备（`beam-encode` 的 `open_native`），因此编码器与抓屏天然同设备；
//! - 客户端侧则是另一条链：解码纹理必须建在渲染层交换链的设备上
//!   （`beam-client` 的 `open_decoder_on_renderer_device`）。
//!
//! 主机与客户端是**两台机器**，两条链各自成立，不存在「主机设备 = 客户端设备」。
//!
//! > **待补接口**：目前 `beam-capture` / `beam-encode` 都没有公开的
//! > `device()` 访问器，因此本 crate **无法**在运行期断言
//! > 「抓屏设备 == 编码设备」。精确签名见批注与交付报告。

use std::sync::{Arc, Mutex};

use beam_capture::{CaptureConfig, CaptureSource};
use beam_encode::{CapturedFrame as EncodeCapturedFrame, VideoEncoder};
use beam_input::InputSink as PlatformInputSink;
use beam_net::VideoSender as NetVideoSenderTrait;
use beam_proto::{ControlMsg, EncodedFrame, InputEvent};
use beam_session::{FramePipeline, InputSink, SessionError, VideoSender};

/// 空输入接收器：**占位**，主机编排层会把它换成**真实**注入器。
///
/// ## 为什么需要它
///
/// 主机侧要接收客户端的输入流（§5 输入 stream），注入由**平台注入器**
/// （`beam_input::InputInjector` / `SendInputSink`）完成。但 §4 规定平台 crate 只能被
/// host / client 引用 —— 桌面端（`apps/desktop`）**不能**直接依赖
/// `beam-input`，所以它拿不到注入器。
///
/// 因此本 crate 提供一个「占位」：桌面端把它传给主机编排层，主机编排层看到
/// [`beam_input::InputSink::is_placeholder`] 为真，就用**真实注入器**
/// （有界通道 + 专用注入线程 + MMCSS，权限按设备映射）替换它。
///
/// ## 名字的历史
///
/// 本类型以前叫「空接收器」，语义是「读走输入流但**不**注入」。
/// 现在 `beam-input` 的真实注入器已经落地（上一批 B6 做了 `SendInput`，
/// 本批做了注入线程与权限映射），语义变成「读走输入流并**由主机注入**」。
/// 名字保留是为了不破坏桌面端的调用点（`apps/desktop` 不在本次改动范围内）。
///
/// 想要「读走但不注入」的调用方可以自己实现 `beam_input::InputSink`
/// （默认 `is_placeholder() == false`，主机编排层会原样使用它）。
#[derive(Debug, Clone, Default)]
pub struct NoopInputSink;

impl NoopInputSink {
    /// 新建一个空输入接收器（占位）。
    pub fn new() -> Self {
        Self
    }
}

impl beam_input::InputSink for NoopInputSink {
    fn apply(&mut self, ev: InputEvent) -> Result<(), beam_input::InputError> {
        // 占位注入器**自己**不注入；主机编排层会先用真实注入器替换它。
        // 万一被直接使用（例如脱离主机编排层），这里也**不返回错误**：
        // 输入流本身是好的，只是本机没有可用的注入器。
        let _ = ev;
        Ok(())
    }

    /// 本类型是**占位**（§4：调用方拿不到平台注入器）。
    fn is_placeholder(&self) -> bool {
        true
    }
}

///
/// ## 为什么需要它
///
/// 主机的「监听 → 握手 → 能力协商 → 控制面」这条链路**不依赖画面**：
/// 配对闸门（产品方案 §8 的人工确认）、能力协商、控制面消息全都跑在
/// QUIC 的控制流上。只有「画面」才需要平台后端（WGC 抓屏 + NVENC 硬编）。
///
/// 桌面端在**没有**打开 `platform` feature 时（`apps/desktop` 的默认构建、
/// 以及 CI 的 `cargo check`），拿不到平台后端。如果此时连主链路也不接，
/// 「已开播的主机不会真的接受客户端接入」这个缺口就补不上。
///
/// 因此这里提供一个**诚实**的空流水线：
///
/// - 控制面、配对闸门、信任库写入 —— **真的**跑起来；
/// - 画面 —— 一帧都不产出，**不伪造**。
///
/// 打开 `platform` feature 后，桌面端改用
/// [`crate::platform::PlatformPipeline`]（WGC + NVENC 真后端），
/// 本类型只是那条路径的降级替身。
#[derive(Debug, Clone, Default)]
pub struct EmptyPipeline {
    errors: PipelineErrors,
}

impl EmptyPipeline {
    /// 新建一条空流水线。
    pub fn new() -> Self {
        Self::default()
    }

    /// 错误槽（与真实流水线同形；空流水线本身不会产生错误）。
    pub fn errors(&self) -> PipelineErrors {
        self.errors.clone()
    }
}

impl FramePipeline for EmptyPipeline {
    fn start(&mut self) -> Result<(), SessionError> {
        // 没有画面可启动，也**不**假装启动成功：
        // 控制面照常工作，画面通道保持静默。
        Ok(())
    }

    fn poll(&mut self) -> Option<EncodedFrame> {
        None
    }

    fn request_idr(&mut self) {
        // 没有编码器，自然没有 IDR 可请求。客户端会等到真正有画面时才拿到 IDR。
    }

    fn stop(&mut self) {}
}

/// 主机侧流水线的错误槽（§3.9）。
///
/// `beam-session` 的 [`FramePipeline::poll`] 没有错误返回通道，
/// 因此**真实**的编码错误（例如 `E6001` 没有可用硬编）由本 crate 记在这里，
/// 由调用方通过 [`crate::runtime::HostSessionHandle::take_error`] /
/// [`crate::runtime::HostSessionHandle::pump`] 取走并让会话进入错误态。
///
/// 注意：**「本轮暂无输出」（`E6005`）不是错误**，不进这个槽。
#[derive(Debug, Clone, Default)]
pub struct PipelineErrors {
    inner: Arc<Mutex<Option<SessionError>>>,
}

impl PipelineErrors {
    /// 记一个错误（保留最早的一个，避免后来的覆盖掉根因）。
    pub fn record(&self, err: SessionError) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(err);
        }
    }

    /// 取走最近一次错误。
    pub fn take(&self) -> Option<SessionError> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// 是否有待取走的错误。
    pub fn has_error(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

/// 抓屏 → 编码流水线适配器。
///
/// §4 规定 beam-encode 不能依赖 beam-capture，因此两边的 `CapturedFrame`
/// 是同形但独立的类型，这里做一次字段搬运。
#[derive(Debug)]
pub struct CaptureEncodePipeline<C, E>
where
    C: CaptureSource,
    E: VideoEncoder,
{
    capture: C,
    encoder: E,
    capture_cfg: CaptureConfig,
    /// 编码器输出帧的尺寸（宽, 高）。
    ///
    /// 抓屏纹理自己的真实尺寸需要 `ID3D11Texture2D::GetDesc`（unsafe），
    /// 而本 crate 是 `#![forbid(unsafe_code)]`；`beam_capture::CapturedFrame`
    /// 目前也不带宽高。因此这里用**配置的串流分辨率**，
    /// 等 `beam-capture` 补上宽高字段后再换成纹理真值。
    frame_size: (u32, u32),
    /// 真实错误的去处（`E6005` 之类的「暂无输出」不进这里）。
    errors: PipelineErrors,
}

impl<C, E> CaptureEncodePipeline<C, E>
where
    C: CaptureSource,
    E: VideoEncoder,
{
    /// 组装适配器（输出尺寸取 1920×1080）。
    pub fn new(capture: C, encoder: E, capture_cfg: CaptureConfig) -> Self {
        Self::with_frame_size(capture, encoder, capture_cfg, (1920, 1080))
    }

    /// 组装适配器并指定输出尺寸（宽, 高）。
    ///
    /// 尺寸取自串流配置（§3.14 → §3.9）；抓屏纹理的真实尺寸需要
    /// `beam-capture` 把宽高带出来（见模块文档的「待补接口」）。
    pub fn with_frame_size(
        capture: C,
        encoder: E,
        capture_cfg: CaptureConfig,
        frame_size: (u32, u32),
    ) -> Self {
        Self {
            capture,
            encoder,
            capture_cfg,
            frame_size,
            errors: PipelineErrors::default(),
        }
    }

    /// 访问底层抓屏源。
    pub fn capture(&self) -> &C {
        &self.capture
    }

    /// 访问底层编码器。
    pub fn encoder(&self) -> &E {
        &self.encoder
    }

    /// 错误槽：真实编码错误从这里冒泡到调用方。
    pub fn errors(&self) -> PipelineErrors {
        self.errors.clone()
    }
}

impl<C, E> FramePipeline for CaptureEncodePipeline<C, E>
where
    C: CaptureSource + Send,
    E: VideoEncoder + Send,
{
    fn start(&mut self) -> Result<(), SessionError> {
        self.capture
            .start(self.capture_cfg.clone())
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("抓屏启动失败 {}: {e}", e.code_str()),
            })
    }

    fn poll(&mut self) -> Option<EncodedFrame> {
        let frame = self.capture.latest()?;
        let input = EncodeCapturedFrame {
            tex: frame.tex,
            ts_us: frame.ts_us,
            dirty: frame.dirty,
            width: self.frame_size.0,
            height: self.frame_size.1,
        };
        match self.encoder.encode(&input) {
            Ok(encoded) => Some(encoded),
            // §3.6 `E6005`：硬件编码器有**一帧延迟**——第一次 `encode()` 时
            // 缓冲里还没有码流。这是**正常状态**，不是故障：
            // 显式容忍它，当作「本轮暂无输出」，而不是把它当成失败。
            //
            // 取舍：这里**不**在主机侧再缓存一帧来抹平延迟。
            // 抹平延迟会让主机比真实画面多持有一帧，反而把延迟做大；
            // 而 §4.1 的时序允许首帧晚一点到（客户端会等到 IDR）。
            Err(e) if e.code() == beam_encode::error::code::NO_OUTPUT_YET => {
                tracing::trace!(code = %e.code_str(), "编码器尚未产出帧(有一帧延迟)");
                None
            }
            // 其它错误是真故障：记进错误槽，由上层取走并让会话进入错误态。
            // **不静默吞掉**——调用方必须能区分「还没产出」与「真出错」。
            Err(e) => {
                tracing::error!(code = %e.code_str(), "编码失败");
                self.errors.record(SessionError::TransportFailed {
                    reason: format!("编码失败 {}: {e}", e.code_str()),
                });
                None
            }
        }
    }

    fn request_idr(&mut self) {
        self.encoder.request_idr();
    }

    fn stop(&mut self) {
        self.capture.stop();
    }
}

/// 网络发送适配器。
#[derive(Debug)]
pub struct NetVideoSender<S> {
    sender: S,
}

impl<S> NetVideoSender<S> {
    /// 组装适配器。
    pub fn new(sender: S) -> Self {
        Self { sender }
    }

    /// 访问底层发送器。
    pub fn inner(&self) -> &S {
        &self.sender
    }
}

impl<S> VideoSender for NetVideoSender<S>
where
    S: NetVideoSenderTrait + Send,
{
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        NetVideoSenderTrait::send_frame(&self.sender, f).map_err(|e| {
            SessionError::TransportFailed {
                reason: format!("发送画面失败 {}: {e}", e.code_str()),
            }
        })
    }

    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        // 控制消息由 QUIC 可靠流承载，这里只做适配占位。
        let _ = msg;
        Ok(())
    }
}

/// 输入注入适配器。
#[derive(Debug)]
pub struct InputAdapter<S> {
    sink: S,
}

impl<S> InputAdapter<S> {
    /// 组装适配器。
    pub fn new(sink: S) -> Self {
        Self { sink }
    }

    /// 访问底层注入器。
    pub fn inner(&self) -> &S {
        &self.sink
    }
}

impl<S> InputSink for InputAdapter<S>
where
    S: PlatformInputSink + Send,
{
    fn apply(&mut self, ev: InputEvent) -> Result<(), SessionError> {
        PlatformInputSink::apply(&mut self.sink, ev).map_err(|e| SessionError::TransportFailed {
            reason: format!("注入输入失败 {}: {e}", e.code_str()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_capture::{CapError, CaptureBackend, CapturedFrame};
    use beam_input::{InputError, SendInputSink};
    use beam_proto::{KeyAction, StatsReport};
    use bytes::Bytes;

    struct FakeCapture {
        frames: Vec<Option<CapturedFrame>>,
    }

    impl CaptureSource for FakeCapture {
        fn start(&mut self, _cfg: CaptureConfig) -> Result<(), CapError> {
            Ok(())
        }
        fn latest(&mut self) -> Option<CapturedFrame> {
            if self.frames.is_empty() {
                None
            } else {
                self.frames.remove(0)
            }
        }
        fn stop(&mut self) {}
    }

    struct FakeEncoder {
        idr: bool,
    }

    impl VideoEncoder for FakeEncoder {
        fn encode(
            &mut self,
            f: &EncodeCapturedFrame,
        ) -> Result<EncodedFrame, beam_encode::EncError> {
            Ok(EncodedFrame {
                frame_id: f.ts_us as u32,
                pts_us: f.ts_us,
                capture_ts_us: f.ts_us,
                is_key: true,
                slices: vec![Bytes::from_static(b"x")],
            })
        }
        fn request_idr(&mut self) {
            self.idr = true;
        }
        fn set_bitrate(&mut self, _bps: u32) {}
    }

    struct FakeNetSender {
        sent: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl NetVideoSenderTrait for FakeNetSender {
        fn send_frame(&self, _f: EncodedFrame) -> Result<(), beam_net::NetError> {
            self.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn pipeline_adapts_capture_to_encoder() {
        let capture = FakeCapture { frames: vec![] };
        let encoder = FakeEncoder { idr: false };
        let mut pipeline = CaptureEncodePipeline::new(
            capture,
            encoder,
            CaptureConfig {
                backend: CaptureBackend::Auto,
                display_id: None,
                min_update_interval: None,
                target_fps: 60,
            },
        );
        assert!(pipeline.start().is_ok());
        assert!(pipeline.poll().is_none());
        pipeline.request_idr();
        assert!(pipeline.encoder().idr);
    }

    #[test]
    fn input_adapter_maps_errors() {
        let mut adapter = InputAdapter::new(SendInputSink::new());
        let err = adapter
            .apply(InputEvent::Key {
                code: 4,
                action: KeyAction::Down,
            })
            .expect_err("无权限必须失败");
        assert_eq!(err.code(), 9005);
        let _ = StatsReport::default();
    }

    #[test]
    fn net_sender_adapter_forwards() {
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut adapter = NetVideoSender::new(FakeNetSender {
            sent: std::sync::Arc::clone(&sent),
        });
        assert!(adapter
            .send_frame(EncodedFrame {
                frame_id: 1,
                pts_us: 0,
                capture_ts_us: 0,
                is_key: true,
                slices: vec![Bytes::from_static(b"x")],
            })
            .is_ok());
        assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn input_error_code_is_stable() {
        assert_eq!(InputError::ViGEmNotInstalled.code(), 8001);
    }

    #[test]
    fn noop_sink_is_marked_as_a_placeholder() {
        // 占位标记：主机编排层看到它就会换成**真实**注入器（§3.9）。
        let sink = NoopInputSink::new();
        assert!(
            beam_input::InputSink::is_placeholder(&sink),
            "NoopInputSink 必须是占位,否则桌面端永远拿不到真注入器"
        );
        // 真注入器不是占位。
        let real = SendInputSink::new();
        assert!(!beam_input::InputSink::is_placeholder(&real));
    }
}
