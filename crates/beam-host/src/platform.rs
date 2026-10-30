//! 平台会话工厂与监听编排（§3.9 主机编排 + §3.5/§3.6 平台后端）。
//!
//! ## 这一层存在的理由
//!
//! §4 规定「平台 crate（capture / encode / input / decode / render）只能被
//! host / client 引用」。桌面端（`apps/desktop`）因此**不能**直接依赖
//! `beam-capture` / `beam-encode`——它必须经由 `beam-host` 才能拿到真实后端。
//!
//! 所以本模块（`platform` feature）在本 crate **内部**做三件事：
//!
//! 1. 构造 WGC 抓屏源（§3.5）与 NVENC 硬编编码器（§3.6）；
//! 2. 把它们组装成一条平台流水线；
//! 3. 把流水线交给 [`crate::runtime::HostRuntime::start_quic_session`]，
//!    完成「一条已建立的 QUIC 连接 → 一台真的能串流的主机」。
//!
//! ## 为什么编码在专用 OS 线程上
//!
//! `beam-encode` 的 ffmpeg 后端持有裸指针（`AVCodecContext` 等），
//! 因此 `HardwareVideoEncoder` **不是 `Send`**；而会话层的
//! [`beam_session::FramePipeline`] 要求 `Send`（§3.9「通过 trait 注入，便于测试」）。
//! §6 本身也写着「抓屏 / 编码 / 解码 / 渲染 / 注入用专用 OS 线程」。
//! 两条合起来就是：**抓屏与编码都待在一条固定线程里，会话侧只持有通道**。
//!
//! 这样也顺带满足了 §4.1 的时序要求：编码不在会话线程上做，
//! `poll()` 不会因为一次 NVENC 编码（3–6 ms）而卡住控制面。
//!
//! ## unsafe 策略（决策 D1）
//!
//! 本 crate 仍然是 `#![forbid(unsafe_code)]`：
//! 这里**只调用**平台 crate 的安全 API，
//! 裸指针 / COM 的部分留在 `beam-capture` / `beam-encode` 内部
//! （那两个 crate 自己局部 `#[allow(unsafe_code)]`）。

use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use beam_capture::{CaptureConfig, CaptureSource, WgcCaptureSource};
use beam_encode::{EncodeConfig, HardwareVideoEncoder, VideoEncoder};
use beam_input::InputSink as PlatformInputSink;
use beam_proto::EncodedFrame;
use beam_session::{FramePipeline, SessionError};

use crate::adapters::PipelineErrors;
use crate::net::HostListener;
use crate::runtime::{HostError, HostRuntime, HostSessionHandle};

/// 给平台线程的指令。
///
/// 只放**不携带平台对象**的指令：画面纹理不跨线程传递
/// （抓屏与编码在同一条线程上完成，纹理始终留在本线程）。
enum PipelineCommand {
    /// 请求一个 IDR（§3.5 `request_idr`、§4.1「重连后主机发 IDR」）。
    RequestIdr,
    /// 停止流水线（§3.9 会话关闭）。
    Stop,
}

/// 平台流水线：抓屏（§3.5）+ 硬编（§3.6），跑在一条专用 OS 线程上（§6）。
///
/// 会话侧只持有两条通道，因此它是 `Send` 的（§3.9 `FramePipeline: Send`）。
pub struct PlatformPipeline {
    /// 编码线程产出的码流。
    encoded: Receiver<EncodedFrame>,
    /// 送给编码线程的指令。
    commands: Sender<PipelineCommand>,
    /// 真实错误的去处（`E6005` 之类的「还没产出」不进这里）。
    errors: PipelineErrors,
    /// 输出尺寸（宽, 高）。
    frame_size: (u32, u32),
}

impl std::fmt::Debug for PlatformPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformPipeline")
            .field("frame_size", &self.frame_size)
            .field("has_pipeline_error", &self.errors.has_error())
            .finish()
    }
}

impl PlatformPipeline {
    /// 组装平台流水线（此时还没有启动任何线程）。
    ///
    /// 抓屏与编码都在 [`PlatformPipeline::start`] 里那条专用线程上跑。
    pub fn new(
        capture_cfg: CaptureConfig,
        encode_cfg: EncodeConfig,
        frame_size: (u32, u32),
    ) -> Self {
        let (command_tx, command_rx) = std::sync::mpsc::channel::<PipelineCommand>();
        let (encoded_tx, encoded_rx) = std::sync::mpsc::sync_channel::<EncodedFrame>(4);
        let errors = PipelineErrors::default();
        Self {
            encoded: encoded_rx,
            commands: command_tx,
            errors,
            frame_size,
        }
        .spawn_worker(capture_cfg, encode_cfg, command_rx, encoded_tx)
    }

    /// 起那条专用线程：抓屏 + 编码都在上面完成。
    fn spawn_worker(
        self,
        capture_cfg: CaptureConfig,
        encode_cfg: EncodeConfig,
        command_rx: Receiver<PipelineCommand>,
        encoded_tx: SyncSender<EncodedFrame>,
    ) -> Self {
        let errors = self.errors.clone();
        let frame_size = self.frame_size;
        let target_fps = capture_cfg.target_fps.max(1);
        let spawned = std::thread::Builder::new()
            .name("beam-capture-encode".to_string())
            .spawn(move || {
                run_capture_encode(
                    capture_cfg,
                    encode_cfg,
                    command_rx,
                    encoded_tx,
                    errors,
                    frame_size,
                    target_fps,
                );
            });
        if let Err(err) = spawned {
            // 起线程失败：记成真实错误，由上层 `pump()` 冒泡（不静默）。
            self.errors.record(SessionError::TransportFailed {
                reason: format!("起抓屏/编码线程失败: {err}"),
            });
        }
        self
    }

    /// 错误槽：真实错误从这里冒泡到调用方（§3.9）。
    pub fn errors(&self) -> PipelineErrors {
        self.errors.clone()
    }
}

/// 抓屏 + 编码线程的主体。
///
/// 这条线程**独占** `WgcCaptureSource` 与 `HardwareVideoEncoder`：
/// 纹理不跨线程传递，编码器也不需要 `Send`。
fn run_capture_encode(
    capture_cfg: CaptureConfig,
    encode_cfg: EncodeConfig,
    command_rx: Receiver<PipelineCommand>,
    encoded_tx: SyncSender<EncodedFrame>,
    errors: PipelineErrors,
    frame_size: (u32, u32),
    target_fps: u32,
) {
    let mut capture = WgcCaptureSource::new();
    let mut encoder = match HardwareVideoEncoder::open(encode_cfg) {
        Ok(encoder) => encoder,
        Err(err) => {
            errors.record(SessionError::TransportFailed {
                reason: format!("打开硬编失败 {}: {err}", err.code_str()),
            });
            return;
        }
    };
    if let Err(err) = capture.start(capture_cfg) {
        errors.record(SessionError::TransportFailed {
            reason: format!("抓屏启动失败 {}: {err}", err.code_str()),
        });
        return;
    }

    let frame_interval = Duration::from_micros(1_000_000 / u64::from(target_fps));
    let mut next_deadline = Instant::now();
    loop {
        // 指令优先（IDR / 停止）。
        while let Ok(command) = command_rx.try_recv() {
            match command {
                PipelineCommand::RequestIdr => encoder.request_idr(),
                PipelineCommand::Stop => {
                    capture.stop();
                    return;
                }
            }
        }

        // 抓一帧、编一帧。
        if let Some(frame) = capture.latest() {
            let input = beam_encode::CapturedFrame {
                tex: frame.tex,
                ts_us: frame.ts_us,
                dirty: frame.dirty,
                width: frame_size.0,
                height: frame_size.1,
            };
            match encoder.encode(&input) {
                Ok(encoded) => {
                    // 客户端来不及消费时丢掉这一帧（客户端会等到下一个 IDR）。
                    // 这**不是**错误：背压不是故障。
                    if let Err(TrySendError::Full(_)) = encoded_tx.try_send(encoded) {
                        tracing::debug!("客户端消费不过来,丢弃一帧(背压,非错误)");
                    }
                }
                // §3.6 `E6005`：硬件编码器有一帧延迟,第一次 `encode()` 还没有码流。
                // 这是**正常状态**,不是故障,不进错误槽。
                Err(err) if err.code() == beam_encode::error::code::NO_OUTPUT_YET => {
                    tracing::trace!(code = %err.code_str(), "编码器尚未产出帧(有一帧延迟)");
                }
                // 其它错误是真故障:记进错误槽,由上层取走并让会话进入错误态。
                Err(err) => {
                    tracing::error!(code = %err.code_str(), "编码失败");
                    errors.record(SessionError::TransportFailed {
                        reason: format!("编码失败 {}: {err}", err.code_str()),
                    });
                }
            }
        }

        // 节拍（§3.5 抓屏节拍 = 目标帧率）。
        next_deadline += frame_interval;
        let now = Instant::now();
        if next_deadline > now {
            std::thread::sleep(next_deadline - now);
        } else {
            // 落后太多就重新对齐,不要追赶式地空转。
            next_deadline = now;
        }
    }
}

impl FramePipeline for PlatformPipeline {
    fn start(&mut self) -> Result<(), SessionError> {
        // 线程在构造时就已经起来了；这里没有额外的启动动作。
        // 真实错误（编码器打不开 / 抓屏起不来）由错误槽冒泡。
        Ok(())
    }

    fn poll(&mut self) -> Option<EncodedFrame> {
        self.encoded.try_recv().ok()
    }

    fn request_idr(&mut self) {
        let _ = self.commands.send(PipelineCommand::RequestIdr);
    }

    fn stop(&mut self) {
        let _ = self.commands.send(PipelineCommand::Stop);
    }
}

impl HostRuntime {
    /// 用**真实平台后端**启动主机会话：WGC 抓屏（§3.5）+ NVENC 硬编（§3.6）。
    ///
    /// 通道分工与 [`crate::runtime::HostRuntime::start_quic_session`] 一致：
    /// 控制 stream 0（双向可靠）、画面 datagram、输入 uni stream（§5）。
    ///
    /// 失败情形：
    ///
    /// - 本机没有可用硬编 → `E6001`（记进错误槽，由 `pump()` 冒泡）；
    /// - 抓屏后端不可用 → `E5001` / `E5002`（同上）。
    ///
    /// 返回的句柄需要调用方自己 `pump()`：
    /// 真实错误（不是「编码器还没出帧」的 `E6005`）会从 `pump()` 返回。
    pub async fn start_platform_session<I>(
        &self,
        conn: beam_net::Conn,
        input: Option<I>,
    ) -> Result<HostSessionHandle, HostError>
    where
        I: PlatformInputSink + Send + 'static,
    {
        // §3.5：抓屏配置（`auto` 后端：先 WGC，独占全屏时按规格切 DXGI）。
        // §3.6：硬编配置（`auto` 后端：NVENC → AMF → QSV）。
        let pipeline = PlatformPipeline::new(
            self.config().capture.clone(),
            self.config().encode.clone(),
            (
                u32::from(self.config().stream.width),
                u32::from(self.config().stream.height),
            ),
        );
        let errors = pipeline.errors();
        self.start_quic_session_with_pipeline(conn, Box::new(pipeline), errors, input)
            .await
    }
}

impl HostListener {
    /// 接受一条客户端连接，并用平台后端起一个主机会话。
    ///
    /// 这是 §3.9 的**完整编排入口**：`listen → accept → start_quic_session`。
    /// 调用方拿到句柄后自己 `pump()`（真实错误从 `pump()` 冒泡）。
    ///
    /// `input` 为 `None` 时不接收输入流（客户端不注入键鼠）。
    pub async fn accept_platform_session<I>(
        &self,
        runtime: &HostRuntime,
        input: Option<I>,
    ) -> Result<HostSessionHandle, HostError>
    where
        I: PlatformInputSink + Send + 'static,
    {
        let conn = self.accept().await?;
        runtime.start_platform_session(conn, input).await
    }
}
