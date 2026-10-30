//! 主机侧 QUIC 传输适配器（§3.2 网络层 / §5 三通道）。
//!
//! ## 为什么在这一层
//!
//! 主机侧的 QUIC 接受端原本寄居在 `beam-client`（客户端 crate 承载主机职责，
//! 属职责倒挂）。本模块把它下沉到 `beam-host`：
//!
//! | 通道 | 方向 | QUIC 原语 | 本模块的类型 |
//! |---|---|---|---|
//! | 控制 stream 0 | 双向可靠 | `accept_bi` | [`HostTransport`] |
//! | 输入 stream | 客户端 → 主机，单向可靠 | `accept_uni` | [`HostInput`] |
//! | 视频 / 音频 | 主机 → 客户端，datagram | `send_datagram` | [`HostTransport`]（`beam_net::VideoSender`） |
//!
//! ## 线上帧格式
//!
//! 与客户端侧对称：**4 字节大端长度前缀 + postcard 负载**
//! （负载自带 1 字节消息族标签，由 `beam_proto::encode` 产生）。
//! 这个长度前缀是实现约定，§5 未规定。
//!
//! ## 分帧编解码的唯一实现
//!
//! §4 的分层规则不允许 L3 的 crate 之间横向依赖（`host` 与 `client` 同层），
//! 所以这段分帧编解码**曾经**在两端各存一份 —— 改了一边忘了另一边，
//! 线上直接不通且极难发现。
//!
//! 现在它收敛到了 [`beam_net::framing`]（`beam-net` 是 L2 网络层，
//! `beam-host` 与 `beam-client` 都依赖它，**不构成** L3 横向依赖）。
//! 本模块只调用 [`beam_net::framing`] 的 API，**不再**自己实现分帧。

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use beam_net::framing::{decode_payload, encode_frame, FrameDecoder};
use beam_pair::PeerVerifier;
use beam_proto::{ControlMsg, EncodedFrame, InputEvent};
use beam_session::{ControlChannel, SessionError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

use crate::runtime::HostError;

/// 把网络层错误映射成本层的传输错误。
///
/// **错误码保持不变**：分帧失败在本层一直报 `SessionError::TransportFailed`
/// （E9005），改用 `beam_net::framing` 之后底层错误是 E2005，
/// 但对外表现仍然是 E9005 —— 收敛的是**实现**，不是**对外语义**。
fn framing_err(err: beam_net::NetError) -> SessionError {
    SessionError::TransportFailed {
        reason: format!("{}: {}", err.code_str(), err.text()),
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

/// 非阻塞写：把待发字节尽量写进流里。
///
/// 返回 `Pending`（发送缓冲满）时保留剩余字节，等待下一次调用继续写。
fn flush_stream(
    out: &mut OutBuf,
    writer: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<(), SessionError> {
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
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

/// 主机侧 QUIC 传输适配器。
///
/// - 控制通道：§5 控制 stream 0（双向可靠流），承载 [`ControlMsg`]；
/// - 画面：§5 QUIC datagram，本类型实现 [`beam_net::VideoSender`]；
/// - 输入：§5 输入 stream（客户端 → 主机），由 [`HostInput`] 单独接收。
///
/// 客户端侧的对称实现（`beam_client::NetTransport`）在 `beam-client`。
pub struct HostTransport {
    conn: beam_net::Conn,
    ctrl_send: Box<dyn AsyncWrite + Unpin + Send>,
    ctrl_recv: Box<dyn AsyncRead + Unpin + Send>,
    ctrl_out: OutBuf,
    ctrl_in: FrameDecoder,
    /// 构造失败时的原因（首次使用时上报）。
    init_error: Option<String>,
}

impl std::fmt::Debug for HostTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostTransport")
            .field("init_error", &self.init_error)
            .finish()
    }
}

impl HostTransport {
    /// 主机侧：接受控制双向流（§5 控制 stream 0）。
    ///
    /// 注意：QUIC 的流只有在收到对端数据后才对本端可见，
    /// 因此本函数会等到客户端在控制流上写入第一条数据才返回。
    pub async fn accept(conn: beam_net::Conn) -> Result<Self, SessionError> {
        let (send, recv) = conn
            .accept_bi()
            .await
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("接受控制流失败 {}: {e}", e.code_str()),
            })?;
        Ok(Self {
            conn,
            ctrl_send: Box::new(send),
            ctrl_recv: Box::new(recv),
            ctrl_out: OutBuf::default(),
            ctrl_in: FrameDecoder::default(),
            init_error: None,
        })
    }

    /// 访问底层连接。
    pub fn conn(&self) -> &beam_net::Conn {
        &self.conn
    }

    /// 取出一条已经完整的控制帧并解码。
    fn take_control_frame(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        match self.ctrl_in.next_frame() {
            Ok(Some(frame)) => Some(decode_payload::<ControlMsg>(&frame).map_err(framing_err)),
            Ok(None) => None,
            Err(e) => Some(Err(framing_err(e))),
        }
    }
}

impl ControlChannel for HostTransport {
    /// 发一条控制消息（§5 控制 stream 0）。
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        if let Some(reason) = &self.init_error {
            return Err(SessionError::TransportFailed {
                reason: reason.clone(),
            });
        }
        let frame = encode_frame(&msg).map_err(framing_err)?;
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
            let waker = std::task::Waker::noop();
            let mut cx = Context::from_waker(waker);
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
    fn flush(&mut self) -> Result<(), SessionError> {
        if let Some(reason) = &self.init_error {
            return Err(SessionError::TransportFailed {
                reason: reason.clone(),
            });
        }
        flush_stream(&mut self.ctrl_out, &mut *self.ctrl_send)
    }
}

impl beam_net::VideoSender for HostTransport {
    /// 发一帧画面（QUIC datagram，§5）。
    fn send_frame(&self, f: EncodedFrame) -> Result<(), beam_net::NetError> {
        self.conn.send_frame(f)
    }
}

/// 主机侧 QUIC 监听器（§3.2 `NetEndpoint::listen`）。
///
/// ## 为什么在编排层
///
/// §3.9 的主机运行时负责「listen → accept → start_quic_session」这条链路：
/// 网络层只提供「监听」与「接受一条连接」两个原语，
/// 把它们和「用这条连接起一个主机会话」串起来是**编排层**的事。
///
/// 调用方拿到 [`crate::runtime::HostSessionHandle`] 后自己 `pump()`，
/// 流水线里的真实错误由 `pump()` 冒泡（§3.9 / E6005 那套机制）。
pub struct HostListener {
    endpoint: beam_net::NetEndpoint,
    /// 是否已被 [`HostListener::close`] 关闭。
    closed: AtomicBool,
}

impl std::fmt::Debug for HostListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostListener")
            .field("local_addr", &self.endpoint.local_addr().ok())
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish()
    }
}

impl HostListener {
    /// 开始监听（§3.2）。
    ///
    /// `cfg.identity` 必须带本机证书与私钥（§3.3 自签身份），
    /// 否则返回 `E2007`；双向 TLS 校验由 `verifier` 承担
    /// （主机侧用 [`crate::pairing::HostTrustGate`]，见产品方案 §8）。
    pub async fn listen(
        cfg: beam_net::NetConfig,
        verifier: Arc<dyn PeerVerifier>,
    ) -> Result<Self, HostError> {
        let endpoint = beam_net::NetEndpoint::listen(cfg, verifier)
            .await
            .map_err(|e| HostError::TransportUnavailable {
                reason: format!("{}: {}", e.code_str(), e.text()),
            })?;
        Ok(Self {
            endpoint,
            closed: AtomicBool::new(false),
        })
    }

    /// 本地监听地址（端口为 0 时由系统分配，真实端口从这里读）。
    pub fn local_addr(&self) -> Result<SocketAddr, HostError> {
        self.endpoint
            .local_addr()
            .map_err(|e| HostError::TransportUnavailable {
                reason: format!("{}: {}", e.code_str(), e.text()),
            })
    }

    /// 接受一条客户端连接。
    ///
    /// **单次握手失败不算监听失败**：未信任对端的握手会被
    /// [`crate::pairing::HostTrustGate`] 故意拒绝，这时返回 `Err`，
    /// 调用方应当**继续接受**而不是退出循环。
    /// 只有监听被 [`HostListener::close`] 关掉时才会持续失败。
    pub async fn accept(&self) -> Result<beam_net::Conn, HostError> {
        self.endpoint
            .accept()
            .await
            .map_err(|e| HostError::TransportUnavailable {
                reason: format!("{}: {}", e.code_str(), e.text()),
            })
    }

    /// 监听是否已被关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// 关闭监听（§3.11 `stop_hosting`）。
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.endpoint.close();
    }
}

/// 主机侧输入通道（§5 输入 stream，客户端 → 主机）。
///
/// 输入走**单向可靠流**：客户端 `open_uni`，主机 `accept_uni`。
/// QUIC 的单向流只有在收到对端数据后才对本端可见，
/// 因此 [`HostInput::accept`] 会等到客户端发出第一个输入事件才返回。
pub struct HostInput {
    recv: Box<dyn AsyncRead + Unpin + Send>,
    decoder: FrameDecoder,
    /// 读到的字节是否已经到达流末尾。
    eof: bool,
}

impl std::fmt::Debug for HostInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostInput").field("eof", &self.eof).finish()
    }
}

impl HostInput {
    /// 接受客户端的输入单向流（§5 输入 stream）。
    pub async fn accept(conn: &beam_net::Conn) -> Result<Self, SessionError> {
        let recv = conn
            .accept_uni()
            .await
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("接受输入流失败 {}: {e}", e.code_str()),
            })?;
        Ok(Self {
            recv: Box::new(recv),
            decoder: FrameDecoder::default(),
            eof: false,
        })
    }

    /// 非阻塞地轮询一个输入事件。
    ///
    /// 返回 `None` 表示当前没有新事件（流已结束或暂时没数据）。
    pub fn poll_event(&mut self) -> Option<Result<InputEvent, SessionError>> {
        if self.eof {
            return None;
        }
        let mut raw = [0u8; 4096];
        let filled = {
            let waker = std::task::Waker::noop();
            let mut cx = Context::from_waker(waker);
            let mut buf = ReadBuf::new(&mut raw);
            match Pin::new(&mut *self.recv).poll_read(&mut cx, &mut buf) {
                Poll::Ready(Ok(())) => buf.filled().len(),
                Poll::Ready(Err(e)) => {
                    return Some(Err(SessionError::TransportFailed {
                        reason: format!("读输入流失败: {e}"),
                    }))
                }
                Poll::Pending => 0,
            }
        };
        if filled == 0 {
            self.eof = true;
            return None;
        }
        self.decoder.push(&raw[..filled]);
        self.take_buffered_event()
    }

    /// 等待下一个输入事件（阻塞到有数据或流结束）。
    ///
    /// 返回 `None` 表示输入流已经结束。
    pub async fn next_event(&mut self) -> Option<Result<InputEvent, SessionError>> {
        loop {
            if let Some(item) = self.take_buffered_event() {
                return Some(item);
            }
            if self.eof {
                return None;
            }
            let mut raw = [0u8; 4096];
            let filled = match self.recv.read(&mut raw).await {
                Ok(n) => n,
                Err(e) => {
                    self.eof = true;
                    return Some(Err(SessionError::TransportFailed {
                        reason: format!("读输入流失败: {e}"),
                    }));
                }
            };
            if filled == 0 {
                self.eof = true;
                return None;
            }
            self.decoder.push(&raw[..filled]);
        }
    }

    /// 从已缓冲的字节里切出一条完整输入事件。
    fn take_buffered_event(&mut self) -> Option<Result<InputEvent, SessionError>> {
        match self.decoder.next_frame() {
            Ok(Some(frame)) => Some(decode_payload::<InputEvent>(&frame).map_err(framing_err)),
            Ok(None) => None,
            Err(e) => Some(Err(framing_err(e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 主机侧与客户端侧必须共用**同一份**分帧实现。
    ///
    /// 这条测试的真正目的不是「分帧能工作」（那是 `beam_net::framing` 的职责，
    /// 它自己已经测了），而是**防止主机侧再长出一份私有实现**：
    /// 一旦这里改回自研的分帧器，字节级不兼容就会回来。
    #[test]
    fn host_side_uses_the_shared_framing_implementation() {
        // 主机侧写出去的字节，必须与 `beam_net::framing` 写出来的**完全一致**。
        let msg = ControlMsg::Bye;
        let shared = beam_net::framing::encode_frame(&msg).expect("共享实现编码应当成功");
        let local = encode_frame(&msg).map_err(|e| e).expect("主机侧编码应当成功");
        assert_eq!(local, shared, "主机侧必须与共享实现产出相同字节");

        // 并且能通过共享实现的解码器读回来。
        let mut decoder = FrameDecoder::default();
        decoder.push(&local);
        let got = decoder
            .next_frame()
            .expect("不应出错")
            .expect("应当切出完整帧");
        let decoded = decode_payload::<ControlMsg>(&got).expect("应当能解码");
        assert_eq!(decoded, ControlMsg::Bye);
    }

    /// 分帧失败在本层仍然报 **E9005**（`SessionError::TransportFailed`）。
    ///
    /// 底层换成了 `beam_net::framing`（E2005），但**对外错误码不变** ——
    /// 收敛的是实现，不是对外语义。
    #[test]
    fn oversized_frame_is_rejected_as_transport_failure() {
        let mut decoder = FrameDecoder::default();
        let mut frame = Vec::new();
        frame.extend_from_slice(&(beam_net::framing::MAX_FRAME_LEN as u32 + 1).to_be_bytes());
        decoder.push(&frame);
        let err = framing_err(decoder.next_frame().expect_err("超长帧必须报错"));
        assert_eq!(err.code(), 9005);
    }

    #[test]
    fn input_event_round_trip() {
        let ev = InputEvent::Key {
            code: 42,
            action: beam_proto::KeyAction::Down,
        };
        let frame = encode_frame(&ev).expect("编码应当成功");
        let mut decoder = FrameDecoder::default();
        decoder.push(&frame);
        let got = decoder
            .next_frame()
            .expect("不应出错")
            .expect("应当切出完整帧");
        let decoded = decode_payload::<InputEvent>(&got).expect("应当能解码");
        assert_eq!(decoded, ev);
    }
}
