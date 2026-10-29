//! # beam-session —— Beam 会话层
//!
//! 对应技术设计文档 **§3.9**（模块设计）与 **§4.1**（时序）。
//!
//! ## 职责
//!
//! 主 / 客会话状态机；串联 capture→encode→net、net→decode→render；
//! 能力协商、IDR 请求、重连、统计汇总。
//!
//! ## 状态
//!
//! `Idle → Discovering → Pairing → Connecting → Negotiating → Streaming ⇄ Reconnecting → Closed`；
//! `Error` 可从任意状态进入（§3.9）。
//!
//! ## 接口
//!
//! - [`HostSession::start`]：主机会话。
//! - [`HostSession::start_with_control`]：带控制通道的主机会话（§4.1 时序）。
//! - [`ClientSession::connect`]：客户端会话。
//! - [`ClientSession::connect_with_control`]：带配对 / 码率控制依赖的客户端会话。
//! - [`SessionHandle`]：`events()` / `update()` / `stop()` / `set_clock()` / `agreed()`.
//!
//! ## 控制通道（§4.1 / §5）
//!
//! - 控制面走 [`ControlChannel`]（QUIC 双向可靠流，§5 控制 stream 0）。
//! - 消息处理集中在 [`control::ControlContext`]：能力协商（`Hello` → `HelloAck`）、
//!   配对（`PairRequest` / `PairChallenge` → `PairOk` / `PairFail`）、
//!   `RequestIdr`、`Feedback`、`Resume`、`Bye`。
//! - 超时判定用可注入的 [`Clock`]：单测注入 [`ManualClock`] 即可覆盖
//!   「3 s 停滞 → Reconnecting」「30 s 超时 → Closed」两条判定。
//!
//! ## 边界
//!
//! - **不负责**：不直接调用平台 API，通过 trait（`FramePipeline` / `VideoSender` /
//!   `FrameConsumer` / `InputSink` / `ControlChannel` / `Transport`）注入，便于测试。
//! - **依赖**：proto（数据与协议）、pair（§3.3 配对）、futures（事件流）。
//! - **被依赖**：host、client、ffi。
//! - **禁止**：依赖 capture / encode / decode / render 等平台 crate（§4）。
//!
//! ## 线程模型
//!
//! 状态机本身是同步的，由调用方在自己的任务里推进（`apply` / `pump`）；
//! 事件通过无界通道分发到 UI。
//!
//! ## 错误码
//!
//! `E9xxx`：`9001` 非法状态转移、`9002` 依赖缺失、`9003` 会话超时、
//! `9004` 会话已关闭、`9005` 传输失败。
//!
//! ## 配置
//!
//! `reconnect_timeout=30s`、`stall_threshold=3s`（[`SessionTimeouts`]）。
//!
//! ## 测试
//!
//! 状态机表驱动测试（全部合法与非法转移）+ 控制消息处理单测
//! （能力协商 / 配对 / IDR / Feedback / Resume / Bye）
//! + 假时钟驱动的停滞与重连超时判定测试。

#![forbid(unsafe_code)]

pub mod client;
pub mod clock;
pub mod control;
pub mod error;
pub mod event;
pub mod handle;
pub mod host;
pub mod state;

pub use client::{ClientControlDeps, ClientDeps, ClientSession, ClientSessionConfig, PeerAddr};
pub use clock::{Clock, ManualClock, SystemClock};
pub use control::{
    caps_from_config, config_from_agreed, next_session_id, ControlContext, ControlOutcome,
    ControlRole, PairingConfig, STATS_INTERVAL, STATS_INTERVAL_MS,
};
pub use error::{message, SessionError};
pub use event::{
    ControlChannel, FrameConsumer, FramePipeline, InputSink, RateControl, SessionEvent,
    SessionInput, Transport, TransportFactory, VideoSender,
};
pub use handle::{SessionEventStream, SessionHandle, SessionTimeouts};
pub use host::{HostControlDeps, HostDeps, HostSession, HostSessionConfig};
pub use state::{SessionState, StateMachine, ALL_STATES};
