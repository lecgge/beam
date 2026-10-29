//! 会话事件与注入依赖（§3.9）。
//!
//! §3.9 边界：「不直接调用平台 API，通过 trait(CaptureSource/VideoEncoder/VideoSender/…) 注入，便于测试」。
//! §4 分层：session 只依赖 trait，因此这里定义的是**会话侧**的 trait 镜像，
//! 由 beam-host / beam-client 用平台实现适配。

use beam_proto::{ControlMsg, EncodedFrame, Feedback, InputEvent, StatsReport, StreamConfig};

use crate::error::SessionError;

/// 会话事件（§6：`session` 汇总为 `SessionEvent::Error{code, retryable}`）。
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// 状态变化。
    StateChanged {
        /// 新状态。
        state: crate::state::SessionState,
    },
    /// 配对完成。
    Paired {
        /// 对端指纹（十六进制）。
        fingerprint: String,
    },
    /// 能力协商完成。
    Negotiated {
        /// 最终串流配置。
        config: StreamConfig,
    },
    /// 统计上报（§3.11 `session://stats` 500 ms 节拍）。
    Stats(StatsReport),
    /// 出错（§6：UI 只根据 code 显示文案）。
    Error {
        /// 错误码（`E` 段编号）。
        code: u16,
        /// 是否值得重试。
        retryable: bool,
    },
    /// 会话结束。
    Closed,
}

impl SessionEvent {
    /// 该事件的错误码（没有则 `None`）。
    pub fn code(&self) -> Option<u16> {
        match self {
            SessionEvent::Error { code, .. } => Some(*code),
            _ => None,
        }
    }
}

/// 会话输入（驱动状态机的外部事件）。
#[derive(Debug, Clone, PartialEq)]
pub enum SessionInput {
    /// 开始发现主机（客户端）。
    Discovering,
    /// 开始配对。
    Pairing,
    /// 配对成功。
    Paired {
        /// 对端指纹。
        fingerprint: String,
    },
    /// 开始连接。
    Connecting,
    /// 连接建立。
    Connected,
    /// 能力协商完成。
    Negotiated(StreamConfig),
    /// 开始推流。
    Streaming,
    /// 长时间没有数据（§4.1：3 s 无帧）。
    Stalled,
    /// 重连成功。
    Reconnected,
    /// 失败。
    Failed(u16),
    /// 主动结束。
    Stop,
}

/// 主机侧帧流水线（抓屏 → 编码）。
///
/// 这是 §3.9 里 `CaptureSource` / `VideoEncoder` 的会话侧镜像，
/// 由 beam-host 用平台实现适配。
pub trait FramePipeline: Send {
    /// 启动流水线。
    fn start(&mut self) -> Result<(), SessionError>;
    /// 取最新一帧已编码画面；暂无新帧返回 `None`。
    fn poll(&mut self) -> Option<EncodedFrame>;
    /// 请求关键帧。
    fn request_idr(&mut self);
    /// 停止流水线。
    fn stop(&mut self);
}

/// 视频发送器（§3.9 `VideoSender` 的会话侧镜像）。
pub trait VideoSender: Send {
    /// 发送一帧画面。
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError>;
    /// 发送一条控制消息。
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError>;
}

/// 客户端侧解码与上屏（§3.9 `VideoDecoder` 的会话侧镜像）。
pub trait FrameConsumer: Send {
    /// 消费一帧画面。
    fn consume(&mut self, f: EncodedFrame) -> Result<(), SessionError>;
}

/// 输入注入器（§3.8 `InputSink` 的会话侧镜像）。
pub trait InputSink: Send {
    /// 注入一个输入事件。
    fn apply(&mut self, ev: InputEvent) -> Result<(), SessionError>;
}

/// 控制通道（§5 控制 stream 0，双向可靠）。
///
/// 会话层只认这个 trait，不认具体传输实现（§3.9 边界）：
/// beam-client 用 QUIC 双向流实现它，测试里用假实现替换。
pub trait ControlChannel: Send {
    /// 发送一条控制消息。
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError>;
    /// 轮询下一条控制消息。
    ///
    /// 实现必须是**非阻塞**的：返回 `None` 表示当前没有新消息。
    /// 连接结束同样返回 `None`——会话层靠停滞判定（§4.1「3 s 无数据」）
    /// 感知断链，而不是靠阻塞在读上。
    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>>;
    /// 把尚未写完的字节冲出去。
    ///
    /// 底层发送缓冲满时，消息会留在发送队列里；
    /// 会话层在每次 `pump` 时调用本方法续写。
    /// 默认实现什么都不做（同步实现不需要）。
    fn flush(&mut self) -> Result<(), SessionError> {
        Ok(())
    }
}

/// 码率控制器（§3.2 会话侧镜像）。
///
/// 实现由上层注入（beam-client 适配 `beam_net::RateController`），
/// 会话层只负责把收到的 [`ControlMsg::Feedback`] 喂给它。
pub trait RateControl: Send {
    /// 处理一次网络反馈，返回目标码率（bit/s）。
    fn on_feedback(&mut self, fb: &Feedback) -> u32;
}

/// 会话传输（把网络细节挡在会话层之外）。
pub trait Transport: ControlChannel {
    /// 发送一帧画面。
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError>;
    /// 发送一个输入事件。
    fn send_input(&mut self, ev: InputEvent) -> Result<(), SessionError>;
    /// 接收下一帧画面；`None` 表示当前没有新帧。
    fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>>;
}

/// 传输工厂（由 host / client 层注入 beam-net 的连接逻辑）。
pub trait TransportFactory: Send {
    /// 建立传输。
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_event_exposes_code() {
        let ev = SessionEvent::Error {
            code: 9001,
            retryable: false,
        };
        assert_eq!(ev.code(), Some(9001));
        assert_eq!(SessionEvent::Closed.code(), None);
    }
}
