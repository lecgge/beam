//! 平台实现 → 会话 trait 的适配器（§3.9「通过 trait 注入，便于测试」）。
//!
//! ## QUIC 传输适配器住在 beam-session
//!
//! 客户端侧的 QUIC 传输（控制双向流 + 输入单向流 + 画面 datagram，§5 三通道）
//! 实现的是**会话层自己的 trait**（[`beam_session::Transport`] /
//! [`beam_session::ControlChannel`]），
//! 因此它住在 `beam-session`：[`beam_session::net_transport::NetTransport`]。
//!
//! 为什么要下沉：§4 规定 **L3 之间不得横向依赖** —— `beam-ffi` 不能依赖
//! `beam-client`，但它需要**同一份**传输实现（同一份分帧格式、同一份帧重组语义）。
//! §4 的依赖图允许 `session → net`（L2 → L1），所以实现下沉到会话层，
//! 由本模块 `pub use` 原样重导出——**公开路径 `beam_client::NetTransport` 不变**
//! （桌面端 `apps/desktop/src-tauri` 与 `crates/beam-client/tests/loopback.rs`
//! 都在用这个路径）。
//!
//! ## 通道分工（§5）
//!
//! | 通道 | 承载 | QUIC 原语 | 实现 |
//! |---|---|---|---|
//! | 控制 stream 0 | [`beam_proto::ControlMsg`] | 双向可靠流 | [`NetTransport`] 的 `ctrl_*` |
//! | 输入 stream | [`beam_proto::InputEvent`] | 单向可靠流（客户端 → 主机） | [`NetTransport`] 的 `input*` |
//! | 视频 / 音频 | 帧 | datagram | `beam_net::VideoSender` / `VideoStream` |
//!
//! 线上帧格式：**4 字节大端长度前缀 + postcard 负载**（负载自带 1 字节消息族标签，
//! 由 `beam_proto::encode` 产生）。该格式由 [`beam_net::framing`] **统一实现** ——
//! 本 crate **不再自带分帧实现**，分帧格式的唯一真值来源在 `beam-net`
//! （详见 [`beam_net::framing`] 的模块文档）。
//!
//! 主机侧的对称实现（接受控制流 / 接收输入流）在 `beam-host`：
//! [`NetTransport`] 只负责客户端侧，主机侧适配器不寄居在本 crate。

use beam_decode::VideoDecoder;
use beam_proto::{EncodedFrame, Feedback};
use beam_render::Renderer;
use beam_session::Clock;
use std::sync::Arc;

use beam_session::{
    FrameConsumer, RateControl, SessionError, SystemClock, Transport, TransportFactory,
};

/// 客户端侧的 QUIC 会话传输（实现住在 `beam-session`，此处原样重导出）。
pub use beam_session::{FrameChannelStats, FrameStatsHandle, NetTransport};

/// 解码 → 上屏适配器。
pub struct DecodeRenderConsumer<D, R> {
    decoder: D,
    renderer: R,
    /// 时钟偏移（主机时钟 − 客户端时钟，§5）。
    ///
    /// 会话层每次同步成功后通过 [`FrameConsumer::on_clock_sync`] 送进来；
    /// 算 E2E 延迟（§3.13）时用它把「主机抓屏时刻」换算到客户端时钟。
    clock_offset_us: Option<i64>,
    /// 客户端时钟（微秒）。
    ///
    /// 只有 perf-stats 开启时才会用到（E2E 测量）；关闭时保留字段
    /// 以免两种构建的结构体布局不同。
    #[cfg_attr(not(feature = "perf-stats"), allow(dead_code))]
    clock: Arc<dyn Clock>,
}

impl<D, R> std::fmt::Debug for DecodeRenderConsumer<D, R>
where
    D: VideoDecoder,
    R: Renderer,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 时钟是 `dyn Clock`（没有 Debug），这里只报关键状态。
        f.debug_struct("DecodeRenderConsumer")
            .field("clock_offset_us", &self.clock_offset_us)
            .finish()
    }
}

impl<D, R> DecodeRenderConsumer<D, R>
where
    D: VideoDecoder,
    R: Renderer,
{
    /// 组装适配器。
    pub fn new(decoder: D, renderer: R) -> Self {
        Self {
            decoder,
            renderer,
            clock_offset_us: None,
            clock: Arc::new(SystemClock),
        }
    }

    /// 替换客户端时钟（测试用；生产路径用系统单调时钟）。
    ///
    /// E2E 测量要拿「客户端上屏时刻」，可注入的时钟让这条链路可测。
    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.clock = clock;
    }

    /// 访问底层解码器。
    pub fn decoder(&self) -> &D {
        &self.decoder
    }

    /// 访问底层渲染器。
    pub fn renderer(&self) -> &R {
        &self.renderer
    }

    /// 当前已知的时钟偏移（微秒）。
    pub fn clock_offset_us(&self) -> Option<i64> {
        self.clock_offset_us
    }
}

impl<D, R> FrameConsumer for DecodeRenderConsumer<D, R>
where
    D: VideoDecoder + Send,
    R: Renderer + Send,
{
    fn consume(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        let assembled = beam_proto::AssembledFrame {
            frame_id: f.frame_id,
            pts_us: f.pts_us,
            capture_ts_us: f.capture_ts_us,
            is_key: f.is_key,
            payload: f.to_payload(),
        };
        let decoded =
            self.decoder
                .decode(assembled)
                .map_err(|e| SessionError::TransportFailed {
                    reason: format!("解码失败 {}: {e}", e.code_str()),
                })?;
        self.renderer
            .present(&decoded)
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("上屏失败 {}: {e}", e.code_str()),
            })?;

        // §3.13 的 E2E 阶段：客户端上屏时刻 −（主机抓屏时刻换算到客户端时钟）。
        // 没有时钟偏移就不记 —— 宁可不记，也不能记一个错的数。
        //
        // 整段都挂在 `perf-stats` feature 后面：E2E 是**性能指标**，
        // 没有采集器时无处可放（§7：perf-stats 默认关闭、release 禁止开启）。
        #[cfg(feature = "perf-stats")]
        if let Some(offset_us) = self.clock_offset_us {
            let present_us = self.clock.now_us();
            if let Some(e2e_us) =
                beam_perf::e2e::clock_based_e2e_us(decoded.capture_ts_us, present_us, offset_us)
            {
                beam_perf::collect::record(
                    beam_perf::Stage::E2E,
                    std::time::Duration::from_micros(e2e_us),
                );
            }
        }
        Ok(())
    }

    fn on_clock_sync(&mut self, offset_us: i64) {
        self.clock_offset_us = Some(offset_us);
    }
}

/// 传输工厂适配器：建立一次 QUIC 连接并包装成 [`NetTransport`]。
#[derive(Debug)]
pub struct NetTransportFactory {
    addr: std::net::SocketAddr,
    cfg: beam_net::NetConfig,
    verifier: std::sync::Arc<dyn beam_net::PeerVerifier>,
}

impl NetTransportFactory {
    /// 组装工厂。
    pub fn new(
        addr: std::net::SocketAddr,
        cfg: beam_net::NetConfig,
        verifier: std::sync::Arc<dyn beam_net::PeerVerifier>,
    ) -> Self {
        Self {
            addr,
            cfg,
            verifier,
        }
    }
}

// beam-client 不直接依赖 beam-pair 时用到的类型由调用方注入；
// 这里通过泛型避免额外的 crate 依赖。
impl TransportFactory for NetTransportFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
        let transport = futures::executor::block_on(async {
            let conn =
                beam_net::NetEndpoint::connect(self.addr, self.cfg.clone(), self.verifier.clone())
                    .await
                    .map_err(|e| SessionError::TransportFailed {
                        reason: format!("连接失败 {}: {e}", e.code_str()),
                    })?;
            NetTransport::connect(conn).await
        })?;
        Ok(Box::new(transport))
    }
}

/// 已就绪的传输工厂：把**已经建立好**的传输直接交给会话层。
///
/// 会话层的 [`TransportFactory::connect`] 是同步接口，而 QUIC 建链必须 `await`；
/// 调用方（例如桌面端）先在自己的异步任务里建好 [`NetTransport`]，
/// 再用本工厂注入 [`beam_session::ClientSession`]，
/// 避免在 tokio 运行时里用 `block_on` 阻塞工作线程。
pub struct ReadyTransportFactory {
    transport: Option<Box<dyn Transport>>,
}

impl std::fmt::Debug for ReadyTransportFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadyTransportFactory")
            .field("has_transport", &self.transport.is_some())
            .finish()
    }
}

impl ReadyTransportFactory {
    /// 用已建立的传输组装工厂。
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self {
            transport: Some(transport),
        }
    }
}

impl TransportFactory for ReadyTransportFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
        self.transport
            .take()
            .ok_or_else(|| SessionError::MissingDependency {
                what: "transport".to_string(),
            })
    }
}

/// 把 `beam_net::RateController` 适配成会话层的码率控制器（§3.2）。
///
/// 主机侧收到 `ControlMsg::Feedback` 时由会话层喂给它，
/// 决策结果（目标码率）再回写到串流配置。
#[derive(Debug)]
pub struct NetRateController {
    inner: beam_net::RateController,
}

impl NetRateController {
    /// 用码率上下限构造（§3.2 `bitrate_min` / `bitrate_max`）。
    pub fn new(min_bps: u32, max_bps: u32) -> Self {
        Self {
            inner: beam_net::RateController::new(min_bps, max_bps),
        }
    }

    /// 当前码率。
    pub fn current_bitrate(&self) -> u32 {
        self.inner.current_bitrate()
    }

    /// 底层码率控制器。
    pub fn inner(&self) -> &beam_net::RateController {
        &self.inner
    }
}

impl RateControl for NetRateController {
    fn on_feedback(&mut self, fb: &Feedback) -> u32 {
        self.inner.on_feedback(fb).target_bps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_decode::{D3D11VaDecoder, DecError};
    use beam_net::framing::encode_frame;
    use beam_proto::ControlMsg;
    use beam_render::{D3D11Renderer, RenderError};
    use bytes::Bytes;

    struct FakeDecoder;
    impl VideoDecoder for FakeDecoder {
        fn decode(
            &mut self,
            f: beam_proto::AssembledFrame,
        ) -> Result<beam_proto::DecodedFrame, DecError> {
            Ok(beam_proto::DecodedFrame {
                frame_id: f.frame_id,
                pts_us: f.pts_us,
                capture_ts_us: f.capture_ts_us,
                width: 1920,
                height: 1080,
                is_key: f.is_key,
                surface: beam_proto::PlatformHandle::NONE,
            })
        }
        fn reset(&mut self) -> Result<(), DecError> {
            Ok(())
        }
    }

    struct FakeRenderer {
        presented: usize,
    }
    impl Renderer for FakeRenderer {
        fn present(&mut self, _f: &beam_proto::DecodedFrame) -> Result<(), RenderError> {
            self.presented += 1;
            Ok(())
        }
    }

    #[test]
    fn consumer_decodes_and_presents() {
        let mut consumer = DecodeRenderConsumer::new(FakeDecoder, FakeRenderer { presented: 0 });
        let frame = EncodedFrame {
            frame_id: 1,
            pts_us: 10,
            capture_ts_us: 5,
            is_key: true,
            slices: vec![Bytes::from_static(b"nal")],
        };
        consumer.consume(frame).expect("应当能解码并上屏");
        assert_eq!(consumer.renderer().presented, 1);
    }

    #[test]
    fn consumer_propagates_decode_error() {
        let mut consumer = DecodeRenderConsumer::new(D3D11VaDecoder::new(), D3D11Renderer::new());
        let frame = EncodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            is_key: true,
            slices: vec![Bytes::from_static(b"nal")],
        };
        let err = consumer.consume(frame).expect_err("未启用后端必须失败");
        assert_eq!(err.code(), 9005);
    }

    /// 本 crate **不再自带分帧实现**：分帧格式的唯一真值来源是
    /// [`beam_net::framing`]。这里钉住客户端**实际使用**的编码路径的线上字节 ——
    /// 一旦有人在本 crate 里重新引入第二份分帧实现并改坏格式，本测试会立刻失败。
    #[test]
    fn client_framing_uses_the_shared_beam_net_implementation() {
        let frame = encode_frame(&ControlMsg::Bye).expect("编码应当成功");
        assert_eq!(
            frame,
            vec![0x00, 0x00, 0x00, 0x02, 0x01, 0x0d],
            "客户端控制帧的线上字节必须与 beam_net::framing 一致"
        );
    }

    /// `NetTransport` 仍然能从 `beam_client` 这个公开路径拿到（桌面端在用），
    /// 且它就是会话层下沉的那一个类型（不是第二份实现）。
    #[test]
    fn net_transport_is_the_session_layer_one() {
        assert_eq!(
            std::any::TypeId::of::<NetTransport>(),
            std::any::TypeId::of::<beam_session::NetTransport>(),
            "公开路径必须指向会话层下沉的同一个类型"
        );
    }
}
