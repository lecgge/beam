//! 会话事件与注入依赖（§3.9）。
//!
//! §3.9 边界：「不直接调用平台 API，通过 trait(CaptureSource/VideoEncoder/VideoSender/…) 注入，便于测试」。
//! §4 分层：session 只依赖 trait，因此这里定义的是**会话侧**的 trait 镜像，
//! 由 beam-host / beam-client 用平台实现适配。

use beam_proto::{ControlMsg, EncodedFrame, Feedback, InputEvent, StatsReport, StreamConfig};

use crate::error::SessionError;

/// 帧过期丢弃的原因码（§3.2 **E2004**：帧过期丢弃）。
///
/// 丢帧**不是**会话错误：它不改变会话状态，只作为统计事件上报（§3.11）。
pub const FRAME_EXPIRED_CODE: u16 = 2004;

/// 重连信息（§4.1「3 s 无帧 → Reconnecting」、产品方案 §3 第 5 步）。
///
/// 上层据此显示「重连中」并**保留最后一帧**：
/// - `elapsed_ms`：已经重连了多久（用来显示倒计时 / 进度）；
/// - `keep_last_frame`：是否保留最后一帧画面。
///
/// 通过 [`crate::handle::SessionHandle::reconnecting`] 查询；
/// 不在重连态时返回 `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectingInfo {
    /// 已经重连了多久（毫秒）。
    pub elapsed_ms: u64,
    /// 是否保留最后一帧画面。
    ///
    /// 恒为 `true`：会话层在重连期间**从不**要求清屏，
    /// 最后一帧一直留在屏幕上，直到新帧到达把它覆盖掉。
    pub keep_last_frame: bool,
}

/// 会话事件（§6：`session` 汇总为 `SessionEvent::Error{code, retryable}`）。
///
/// ## 变体顺序
///
/// **新增变体只能加在末尾**（与 `beam-perf` 的 `Stage` 同样的约束）：
/// 变体序号一旦被某处按序号引用就不能重排，否则线上含义会漂移。
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
    /// 丢帧（§3.2 **E2004**：帧过期丢弃）。
    ///
    /// 与 [`SessionEvent::Error`] 的区别：**丢帧不是会话错误**。
    /// 它既不会把会话打进 `Error` 态，也不会让会话结束——
    /// 上层只把它当成一条统计 / 提示（§3.11）。
    ///
    /// 由客户端会话在读到传输层统计到的过期帧时发出（见
    /// [`Transport::take_dropped_frames`]）。
    FramesDropped {
        /// 本次丢弃的帧数。
        count: u32,
        /// 原因码（§3.2 **E2004** = [`FRAME_EXPIRED_CODE`]）。
        code: u16,
    },
}

impl SessionEvent {
    /// 该事件的**会话错误码**；非错误事件（例如丢帧）返回 `None`。
    ///
    /// [`SessionEvent::FramesDropped`] 自带原因码，但它不是会话错误，
    /// 因此这里返回 `None`——避免上层把「丢帧」误当成「会话出错」。
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

/// 客户端侧输入采集源（§3.8）。
///
/// §3.8 的边界写着「**不采集客户端输入**（Windows 客户端由 beam-client 采集，
/// Android 由 `core-input`）」—— 本 trait 就是那条边界在会话层的镜像：
/// 会话层**不认识**任何平台采集后端，只从它这里取事件，
/// 再写进 §5 的**输入单向流**（客户端 → 主机）。
///
/// 真机实现：
/// - Windows：[`beam_client::capture::WindowsInputCapture`]（低级键鼠钩子）；
/// - Android：`core-input`（Kotlin 侧，经 uniffi 上报）。
///
/// 语义：**非阻塞**。暂无新事件返回 `None`（调用方自己决定轮询节奏）。
pub trait InputSource: Send + Sync {
    /// 取下一个输入事件；暂无新事件返回 `None`。
    ///
    /// 用 `&self`（内部可变性）：采集源通常由**采集线程**写入队列、
    /// 由**会话线程**读取，两边共享同一个句柄（`Arc`）。
    fn poll_event(&self) -> Option<InputEvent>;
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

    /// 取走自上次调用以来被流水线丢掉的帧数（§3.5 丢帧统计）。
    ///
    /// 典型来源：抓屏快于编码时，抓屏单槽里的旧帧被覆盖（§3.5）。
    /// 那是**真的丢了**，必须可计量 —— 主机侧的 [`crate::summary::SessionSummary`]
    /// 里的 `dropped_frames` 就来自这里（此前恒为 0）。
    ///
    /// 默认实现返回 `0`：没有丢帧概念的流水线（例如测试替身）。
    fn take_dropped_frames(&mut self) -> u32 {
        0
    }
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

    /// 会话侧算出了时钟偏移（§5「客户端通过 ClockSync 求偏移」），告知消费者。
    ///
    /// 消费者若要做 **E2E 延迟测量**（§3.13），就需要这个偏移把
    /// 「主机抓屏时刻」换算到客户端时钟：
    /// `e2e = 客户端上屏时刻 − (主机抓屏时刻 − 偏移)`。
    ///
    /// 默认实现是**空操作**：不关心 E2E 的消费者（绝大多数）什么都不用做。
    fn on_clock_sync(&mut self, _offset_us: i64) {}

    /// 取走自上次调用以来被消费者丢掉的帧数（§3.10 丢帧统计）。
    ///
    /// 典型来源：解码快于上屏时，渲染单槽里的旧帧被覆盖（§3.10）。
    /// 客户端侧的会话汇总 `dropped_frames` 原本只统计**网络过期帧**
    /// （§3.2 E2004），渲染侧丢帧没算进去 —— 这一条补上。
    ///
    /// 默认实现返回 `0`：不产生丢帧的消费者。
    fn take_dropped_frames(&mut self) -> u32 {
        0
    }
}

/// 输入注入器（§3.8 `InputSink` 的会话侧镜像）。
///
/// ## 归属：输入注入归 **beam-host**
///
/// 输入事件走 §5 的**输入单向流**（客户端 → 主机），由 beam-host 的输入任务
/// （§3.8「输入 stream 接收任务 → 有界通道 → 注入线程」）接收并注入平台。
/// 本 trait 只是会话层给出的**类型边界**：beam-host 的适配器实现它，
/// `beam-session` 自己既不持有也不调用它——会话层没有输入字段
/// （`HostDeps` 里没有 `input`），因此不存在「两处都注入」的可能。
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
    /// 取走自上次调用以来被丢弃的帧数（§3.2 **E2004**：帧过期丢弃）。
    ///
    /// 默认实现返回 `0`（没有丢帧统计的传输实现）。
    /// 会话层据此发出 [`SessionEvent::FramesDropped`]——丢帧不改变会话状态。
    fn take_dropped_frames(&mut self) -> u32 {
        0
    }
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

    #[test]
    fn frames_dropped_is_not_a_session_error() {
        let ev = SessionEvent::FramesDropped {
            count: 3,
            code: FRAME_EXPIRED_CODE,
        };
        assert_eq!(ev.code(), None, "丢帧不是会话错误,不能被当成 Error 处理");
        assert_eq!(FRAME_EXPIRED_CODE, 2004, "E2004 的码值必须稳定");
        assert_eq!(ev, ev.clone(), "事件必须可克隆可比较");
    }
}
