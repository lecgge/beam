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
//! §4 的分层规则不允许 L3 的 crate 之间横向依赖（`host` 与 `client` 同层），
//! 因此这段分帧编解码在两端各存一份，**格式必须保持一致**。

use std::pin::Pin;
use std::task::{Context, Poll};

use beam_proto::{ControlMsg, EncodedFrame, InputEvent, Message};
use beam_session::{ControlChannel, SessionError};
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

/// 线上帧的长度前缀长度（4 字节大端）。
const FRAME_LEN_PREFIX: usize = 4;

/// 单帧上限（防御性上限；控制消息与输入事件都很小）。
const MAX_FRAME_LEN: usize = 64 * 1024;

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

impl ControlChannel for HostTransport {
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
            Ok(Some(frame)) => Some(match beam_proto::decode_as::<InputEvent>(&frame) {
                Ok(ev) => Ok(ev),
                Err(e) => Err(SessionError::TransportFailed {
                    reason: format!("解码输入事件失败: {e}"),
                }),
            }),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_through_length_prefix() {
        let msg = ControlMsg::Bye;
        let frame = encode_frame(&msg).expect("编码应当成功");
        let len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(len, frame.len() - FRAME_LEN_PREFIX);

        let mut decoder = FrameDecoder::default();
        // 先喂一半：应当还切不出完整帧。
        decoder.push(&frame[..2]);
        assert!(decoder.next_frame().expect("不应出错").is_none());
        decoder.push(&frame[2..]);
        let got = decoder
            .next_frame()
            .expect("不应出错")
            .expect("应当切出完整帧");
        let decoded = beam_proto::decode_as::<ControlMsg>(&got).expect("应当能解码");
        assert_eq!(decoded, ControlMsg::Bye);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut decoder = FrameDecoder::default();
        let mut frame = Vec::new();
        frame.extend_from_slice(&(MAX_FRAME_LEN as u32 + 1).to_be_bytes());
        decoder.push(&frame);
        let err = decoder.next_frame().expect_err("超长帧必须报错");
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
        let decoded = beam_proto::decode_as::<InputEvent>(&got).expect("应当能解码");
        assert_eq!(decoded, ev);
    }
}
