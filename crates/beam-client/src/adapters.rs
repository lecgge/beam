//! 平台实现 → 会话 trait 的适配器（§3.9「通过 trait 注入，便于测试」）。
//!
//! ## 通道分工（§5）
//!
//! | 通道 | 承载 | QUIC 原语 | 实现 |
//! |---|---|---|---|
//! | 控制 stream 0 | [`ControlMsg`] | 双向可靠流 | [`NetTransport`] 的 `ctrl_*` |
//! | 输入 stream | [`InputEvent`] | 单向可靠流（客户端 → 主机） | [`NetTransport`] 的 `input*` |
//! | 视频 / 音频 | 帧 | datagram | `beam_net::VideoSender` / `VideoStream` |
//!
//! 线上帧格式：**4 字节大端长度前缀 + postcard 负载**（负载自带 1 字节消息族标签，
//! 由 `beam_proto::encode` 产生）。长度前缀是本层的实现约定，§5 未规定。
//!
//! 主机侧的对称实现（接受控制流 / 接收输入流）在 `beam-host`：
//! [`NetTransport`] 只负责客户端侧，主机侧适配器不寄居在本 crate。
//!
//! ## 帧通道（§5 画面走 datagram）
//!
//! - 画面用 QUIC **datagram** 承载，接收侧是 [`beam_net::VideoStream`]；
//! - [`NetTransport`] **持久持有**一个 `VideoStream`：`Conn::video_stream()` 每次调用
//!   都会新建重组器，轮询时新建会丢掉「已收到但尚未重组完」的分片；
//! - 过期帧（重组超时）按 §3.2 记为 **E2004**：写日志 + 计入
//!   [`FrameChannelStats`]，不静默丢弃。

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use beam_decode::VideoDecoder;
use beam_proto::{ControlMsg, EncodedFrame, Feedback, InputEvent, Message};
use beam_render::Renderer;
use beam_session::{
    ControlChannel, FrameConsumer, RateControl, SessionError, Transport, TransportFactory,
};
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// 线上帧的长度前缀长度（4 字节大端）。
const FRAME_LEN_PREFIX: usize = 4;

/// 单帧上限（防御性上限；控制消息与输入事件都很小）。
const MAX_FRAME_LEN: usize = 64 * 1024;

/// 解码 → 上屏适配器。
#[derive(Debug)]
pub struct DecodeRenderConsumer<D, R> {
    decoder: D,
    renderer: R,
}

impl<D, R> DecodeRenderConsumer<D, R>
where
    D: VideoDecoder,
    R: Renderer,
{
    /// 组装适配器。
    pub fn new(decoder: D, renderer: R) -> Self {
        Self { decoder, renderer }
    }

    /// 访问底层解码器。
    pub fn decoder(&self) -> &D {
        &self.decoder
    }

    /// 访问底层渲染器。
    pub fn renderer(&self) -> &R {
        &self.renderer
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
            })
    }
}

/// 待发送字节缓冲（写侧）。
///
/// QUIC 流的写缓冲可能满，非阻塞写只写入一部分；
/// 剩余字节留在这里，下次发送时继续。
#[derive(Debug, Default)]
struct OutBuf {
    data: Vec<u8>,
    pos: usize,
}

impl OutBuf {
    fn push(&mut self, bytes: &[u8]) {
        if self.pos >= self.data.len() {
            self.data.clear();
            self.pos = 0;
        }
        self.data.extend_from_slice(bytes);
    }

    fn pending(&self) -> &[u8] {
        &self.data[self.pos..]
    }

    fn consume(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n).min(self.data.len());
        if self.pos >= self.data.len() {
            self.data.clear();
            self.pos = 0;
        }
    }

    fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }
}

/// 增量解码器：从字节流里切出完整帧。
#[derive(Debug, Default)]
struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// 取出一条完整帧；数据不够时返回 `Ok(None)`。
    fn next_frame(&mut self) -> Result<Option<Vec<u8>>, SessionError> {
        if self.buf.len() < FRAME_LEN_PREFIX {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_FRAME_LEN {
            return Err(SessionError::TransportFailed {
                reason: format!("线上帧长度超限({len} 字节)"),
            });
        }
        if self.buf.len() < FRAME_LEN_PREFIX + len {
            return Ok(None);
        }
        let frame = self.buf[FRAME_LEN_PREFIX..FRAME_LEN_PREFIX + len].to_vec();
        self.buf.drain(..FRAME_LEN_PREFIX + len);
        Ok(Some(frame))
    }
}

/// 把一条线上消息编码成「4 字节大端长度 + 负载」。
fn encode_frame<M: Message>(msg: &M) -> Result<Vec<u8>, SessionError> {
    let mut buf = BytesMut::new();
    beam_proto::encode(msg, &mut buf).map_err(|e| SessionError::TransportFailed {
        reason: format!("编码失败: {e}"),
    })?;
    let mut out = Vec::with_capacity(FRAME_LEN_PREFIX + buf.len());
    out.extend_from_slice(&(buf.len() as u32).to_be_bytes());
    out.extend_from_slice(&buf);
    Ok(out)
}

/// 非阻塞写：把待发字节尽量写进流里。
///
/// 返回 `Pending`（发送缓冲满）时保留剩余字节，等待下一次调用继续写。
fn flush_stream(
    out: &mut OutBuf,
    writer: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<(), SessionError> {
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        if out.is_empty() {
            break;
        }
        let written = {
            let data = out.pending();
            match Pin::new(&mut *writer).poll_write(&mut cx, data) {
                Poll::Ready(Ok(n)) => n,
                Poll::Ready(Err(e)) => {
                    return Err(SessionError::TransportFailed {
                        reason: format!("写流失败: {e}"),
                    })
                }
                Poll::Pending => break,
            }
        };
        if written == 0 {
            break;
        }
        out.consume(written);
    }
    Ok(())
}

/// 帧通道统计（§3.2）。
///
/// 过期帧（重组超时）按 §3.2 记为 **E2004**，通过本结构暴露给上层，
/// 不做静默丢弃。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameChannelStats {
    /// 已交付给上层的完整帧数。
    pub frames_delivered: u64,
    /// 因超时被丢弃的帧数（E2004）。
    pub frames_expired: u64,
    /// 最近一次被丢弃的帧号。
    pub last_expired_frame_id: Option<u32>,
}

/// 帧通道统计的共享句柄。
///
/// 会话层把传输装箱成 `dyn Transport` 后上层拿不到具体类型，
/// 上层（如桌面端）持有本句柄即可读到 E2004 计数。
#[derive(Debug, Clone, Default)]
pub struct FrameStatsHandle {
    inner: Arc<Mutex<FrameChannelStats>>,
}

impl FrameStatsHandle {
    /// 当前统计快照。
    pub fn snapshot(&self) -> FrameChannelStats {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 基于 QUIC 连接的会话传输适配器（客户端侧）。
///
/// - 控制通道：双向可靠流（§5 控制 stream 0），承载 [`ControlMsg`]；
/// - 输入通道：单向可靠流（§5 输入 stream，客户端 → 主机），承载 [`InputEvent`]；
/// - 画面：QUIC datagram（§5），发送走 `beam_net::VideoSender`，
///   接收走本 crate 持有的 [`beam_net::VideoStream`]。
///
/// 主机侧的对称实现（接受控制流 / 接收输入流）在 `beam-host`。
pub struct NetTransport {
    conn: beam_net::Conn,
    ctrl_send: Box<dyn AsyncWrite + Unpin + Send>,
    ctrl_recv: Box<dyn AsyncRead + Unpin + Send>,
    ctrl_out: OutBuf,
    ctrl_in: FrameDecoder,
    input: Option<Box<dyn AsyncWrite + Unpin + Send>>,
    input_out: OutBuf,
    /// 帧通道：持久持有的视频接收流。
    ///
    /// **必须是持久实例**：`Conn::video_stream()` 每次调用都会新建一个重组器，
    /// 轮询时新建会把「已收到但还没重组完」的分片丢掉，表现为随机丢帧。
    video: beam_net::VideoStream,
    /// 帧通道统计（共享句柄）。
    stats: Arc<Mutex<FrameChannelStats>>,
    /// 构造失败时的原因（首次使用时上报）。
    init_error: Option<String>,
}

impl std::fmt::Debug for NetTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetTransport")
            .field("has_input_stream", &self.input.is_some())
            .field("frame_stats", &self.frame_stats())
            .field("init_error", &self.init_error)
            .finish()
    }
}

impl NetTransport {
    /// 客户端侧：打开控制双向流（§5 控制 stream 0）与输入单向流（§5 输入 stream）。
    pub async fn connect(conn: beam_net::Conn) -> Result<Self, SessionError> {
        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("打开控制流失败 {}: {e}", e.code_str()),
            })?;
        let input = conn
            .open_uni()
            .await
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("打开输入流失败 {}: {e}", e.code_str()),
            })?;
        // 帧通道的接收流只创建一次（`Conn::video_stream()` 每次调用都是新重组器）。
        let video = conn.video_stream();
        Ok(Self {
            conn,
            ctrl_send: Box::new(send),
            ctrl_recv: Box::new(recv),
            ctrl_out: OutBuf::default(),
            ctrl_in: FrameDecoder::default(),
            input: Some(Box::new(input)),
            input_out: OutBuf::default(),
            video,
            stats: Arc::new(Mutex::new(FrameChannelStats::default())),
            init_error: None,
        })
    }

    /// 同步构造（兼容既有 API）：阻塞等待控制流建立。
    ///
    /// 需要在 tokio 运行时上下文里调用；异步上下文请改用 [`NetTransport::connect`]。
    pub fn new(conn: beam_net::Conn) -> Self {
        match futures::executor::block_on(Self::connect(conn.clone())) {
            Ok(t) => t,
            Err(e) => {
                let reason = match e {
                    SessionError::TransportFailed { reason } => reason,
                    other => other.to_string(),
                };
                Self::failed(conn, reason)
            }
        }
    }

    /// 构造一个「一用就报错」的传输（用于把建流失败暴露给会话层）。
    fn failed(conn: beam_net::Conn, reason: String) -> Self {
        let video = conn.video_stream();
        Self {
            conn,
            ctrl_send: Box::new(tokio::io::sink()),
            ctrl_recv: Box::new(tokio::io::empty()),
            ctrl_out: OutBuf::default(),
            ctrl_in: FrameDecoder::default(),
            input: None,
            input_out: OutBuf::default(),
            video,
            stats: Arc::new(Mutex::new(FrameChannelStats::default())),
            init_error: Some(reason),
        }
    }

    /// 访问底层连接。
    pub fn conn(&self) -> &beam_net::Conn {
        &self.conn
    }

    /// 帧通道统计快照（§3.2）。
    pub fn frame_stats(&self) -> FrameChannelStats {
        *self.stats.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 帧通道统计的共享句柄：上层在传输被装箱后仍能读到 E2004 计数。
    pub fn frame_stats_handle(&self) -> FrameStatsHandle {
        FrameStatsHandle {
            inner: Arc::clone(&self.stats),
        }
    }

    /// 把超时丢弃的帧取出来上报（§3.2 **E2004**）。
    ///
    /// 每条过期帧都会写一条 `warn` 日志并累加计数——**不静默丢弃**。
    fn drain_expired(&mut self) {
        let expired = self.video.take_expired();
        if expired.is_empty() {
            return;
        }
        let mut guard = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        for err in expired {
            guard.frames_expired += 1;
            if let beam_net::NetError::FrameExpired { frame_id } = err {
                guard.last_expired_frame_id = Some(frame_id);
                tracing::warn!(
                    code = %err.code_str(),
                    frame_id,
                    "画面帧重组超时并被丢弃(E2004)"
                );
            } else {
                tracing::warn!(code = %err.code_str(), "画面帧通道异常");
            }
        }
    }

    /// 取出一条已经完整的控制帧并解码。
    fn take_control_frame(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        match self.ctrl_in.next_frame() {
            Ok(Some(frame)) => Some(match beam_proto::decode_as::<ControlMsg>(&frame) {
                Ok(msg) => Ok(msg),
                Err(e) => Err(SessionError::TransportFailed {
                    reason: format!("解码控制消息失败: {e}"),
                }),
            }),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

impl ControlChannel for NetTransport {
    /// 发一条控制消息（§5 控制 stream 0）。
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        if let Some(reason) = &self.init_error {
            return Err(SessionError::TransportFailed {
                reason: reason.clone(),
            });
        }
        let frame = encode_frame(&msg)?;
        self.ctrl_out.push(&frame);
        flush_stream(&mut self.ctrl_out, &mut *self.ctrl_send)
    }

    /// 轮询一条控制消息（不阻塞）。
    ///
    /// 返回 `None` 表示当前没有新消息（对端还没发，或流已结束）。
    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        if let Some(reason) = &self.init_error {
            return Some(Err(SessionError::TransportFailed {
                reason: reason.clone(),
            }));
        }
        // 缓冲里已经有完整帧的话直接取。
        if let Some(result) = self.take_control_frame() {
            return Some(result);
        }
        let mut raw = [0u8; 4096];
        let filled = {
            let waker = futures::task::noop_waker();
            let mut cx = Context::from_waker(&waker);
            let mut buf = ReadBuf::new(&mut raw);
            match Pin::new(&mut *self.ctrl_recv).poll_read(&mut cx, &mut buf) {
                Poll::Ready(Ok(())) => buf.filled().len(),
                Poll::Ready(Err(e)) => {
                    return Some(Err(SessionError::TransportFailed {
                        reason: format!("读控制流失败: {e}"),
                    }))
                }
                Poll::Pending => 0,
            }
        };
        if filled == 0 {
            // 对端关闭了控制流（或暂时没数据）：不会再有消息了。
            return None;
        }
        self.ctrl_in.push(&raw[..filled]);
        self.take_control_frame()
    }

    /// 把发送队列里没写完的字节续写出去。
    ///
    /// QUIC 的发送缓冲满时消息会留在队列里，会话层每次 `pump` 调一次本方法。
    fn flush(&mut self) -> Result<(), SessionError> {
        if let Some(reason) = &self.init_error {
            return Err(SessionError::TransportFailed {
                reason: reason.clone(),
            });
        }
        flush_stream(&mut self.ctrl_out, &mut *self.ctrl_send)?;
        if let Some(input) = self.input.as_mut() {
            flush_stream(&mut self.input_out, &mut **input)?;
        }
        Ok(())
    }
}

impl Transport for NetTransport {
    /// 发一帧画面（QUIC datagram，§5）。
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        use beam_net::VideoSender;
        self.conn
            .send_frame(f)
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("发送画面失败 {}: {e}", e.code_str()),
            })
    }

    /// 发一个输入事件（单向可靠流，§5 输入 stream，客户端 → 主机）。
    fn send_input(&mut self, ev: InputEvent) -> Result<(), SessionError> {
        if let Some(reason) = &self.init_error {
            return Err(SessionError::TransportFailed {
                reason: reason.clone(),
            });
        }
        if self.input.is_none() {
            return Err(SessionError::TransportFailed {
                reason: "输入流未建立（主机侧不接受输入）".to_string(),
            });
        }
        let frame = encode_frame(&ev)?;
        self.input_out.push(&frame);
        let writer = self.input.as_mut().expect("输入流已检查");
        flush_stream(&mut self.input_out, &mut **writer)
    }

    /// 接收下一帧画面（QUIC datagram，§5）。
    ///
    /// 语义与 [`beam_net::VideoStream::try_next_frame`] 一致：
    ///
    /// - **不阻塞**：只消费已经到达的数据报；
    /// - **不完整帧会被保留**：`None` 只表示「此刻还没有完整帧」，
    ///   分片留在重组器里等后续分片补齐；
    /// - **过期帧不会被静默丢弃**：重组超时的帧按 §3.2 记为 **E2004**，
    ///   写日志并计入 [`FrameChannelStats`]。
    fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
        if let Some(reason) = &self.init_error {
            return Some(Err(SessionError::TransportFailed {
                reason: reason.clone(),
            }));
        }
        // 先把超时丢弃的帧取出来上报（E2004）。
        self.drain_expired();
        match self.video.try_next_frame() {
            Ok(Some(frame)) => {
                self.stats
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .frames_delivered += 1;
                Some(Ok(EncodedFrame {
                    frame_id: frame.frame_id,
                    pts_us: frame.pts_us,
                    capture_ts_us: frame.capture_ts_us,
                    is_key: frame.is_key,
                    slices: vec![frame.payload],
                }))
            }
            Ok(None) => None,
            Err(e) => Some(Err(SessionError::TransportFailed {
                reason: format!("接收画面失败 {}: {e}", e.code_str()),
            })),
        }
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

    #[test]
    fn frame_round_trip_through_length_prefix() {
        let msg = ControlMsg::Bye;
        let frame = encode_frame(&msg).expect("编码应当成功");
        assert_eq!(frame.len(), FRAME_LEN_PREFIX + 2, "帧头 + 族标签 + 负载");
        let mut decoder = FrameDecoder::default();
        decoder.push(&frame[..frame.len() - 1]);
        assert!(decoder.next_frame().expect("解码不应当失败").is_none());
        decoder.push(&frame[frame.len() - 1..]);
        let out = decoder
            .next_frame()
            .expect("解码不应当失败")
            .expect("应当有帧");
        let back: ControlMsg = beam_proto::decode_as(&out).expect("解码应当成功");
        assert_eq!(back, msg);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut decoder = FrameDecoder::default();
        decoder.push(&(MAX_FRAME_LEN as u32 + 1).to_be_bytes());
        let err = decoder.next_frame().expect_err("超长帧必须失败");
        assert_eq!(err.code(), 9005);
    }

    #[test]
    fn out_buf_tracks_pending_bytes() {
        let mut out = OutBuf::default();
        out.push(b"hello");
        assert_eq!(out.pending(), b"hello");
        out.consume(2);
        assert_eq!(out.pending(), b"llo");
        out.consume(3);
        assert!(out.is_empty());
    }
}
