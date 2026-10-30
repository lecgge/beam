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
//! - [`NetTransport`]：QUIC 会话传输（§5 三通道），L3 的 client / ffi 共用同一份实现。
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
//! - **输入注入归 beam-host**：输入事件走 §5 的输入单向流（客户端 → 主机），
//!   接收与注入都在 beam-host（§3.8 的输入任务）。本 crate **不持有**输入注入器
//!   （[`HostDeps`] 里没有 `input` 字段），因此只有一处会注入输入。
//! - **依赖**：proto（数据与协议）、pair（§3.3 配对）、net（§3.2 QUIC 传输）、
//!   futures（事件流）、tokio（QUIC 的 IO 原语）。
//! - **被依赖**：host、client、ffi。
//! - **禁止**：依赖 capture / encode / decode / render 等平台 crate（§4）。
//!
//! ## 为什么 `net` 依赖落在本层（§4）
//!
//! §4 的依赖图允许 `session → net`（L2 → L1），并规定 **L3 之间不得横向依赖**。
//! QUIC 传输适配器（[`NetTransport`]）实现的是**会话层自己的 trait**
//! （[`Transport`] / [`ControlChannel`]），因此放在本层：
//! L3 的 `beam-client` 与 `beam-ffi` 复用**同一份**分帧 / 传输实现，
//! 不会出现第二份线上格式（§5 的分帧唯一真值来源仍是 `beam_net::framing`）。
//!
//! ## 事件
//!
//! [`SessionEvent`] 的变体**只能加在末尾**（与 `beam-perf` 的 `Stage` 同样的约束）：
//! 变体序号一旦被某处按序号引用就不能重排。
//!
//! - [`SessionEvent::Error`]：会话错误（会把会话打进 `Error` 态）。
//! - [`SessionEvent::FramesDropped`]：丢帧（§3.2 **E2004**）——**不是**会话错误，
//!   只作统计 / 提示；会话状态不变。
//!
//! ## 首帧策略（§4.1）
//!
//! §4.1 写「视频 datagram(首帧 IDR)」：客户端收到的第一帧必须是关键帧。
//! 首帧不是关键帧时，客户端发**一次** `RequestIdr`（见 [`client`] 模块文档）。
//!
//! ## 重连（§4.1、产品方案 §3 第 5 步）
//!
//! - **3 s 无帧 → `Reconnecting`**（[`SessionTimeouts::stall_threshold`]）；
//! - **30 s 内自动重连**：客户端用传输工厂重建一条**新连接**，
//!   重发 `Hello` 重新协商，并补 `Resume{session_id}` 让主机认回同一会话
//!   （**信任已建立，不重走 PIN**）；恢复时请求一次 IDR；
//! - **超时 → `Closed`**（`E9003`）；
//! - 重连期间**保留最后一帧**：[`SessionHandle::reconnecting`] 给出
//!   [`ReconnectingInfo`]（含 `keep_last_frame`）。
//!
//! ## 统计汇总（§3.9）
//!
//! [`SessionHandle::summary`] 给出可查询的会话汇总
//! （帧数 / 丢帧 / RTT / 丢包率 / 帧率 / 码率 / 重连次数），
//! 会话结束后依然能查。
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
//! `reconnect_timeout=30s`、`stall_threshold=3s`（[`SessionTimeouts`]）；
//! 由 [`ClientSessionConfig`] / [`HostSessionConfig`] 注入，**真的生效**
//! （[`client`] / [`host`] 模块的停滞与超时判定读的就是这两个值）。
//!
//! ## 测试
//!
//! 状态机表驱动测试（全部合法与非法转移）+ 控制消息处理单测
//! （能力协商 / 配对 / IDR / Feedback / Resume / Bye）
//! + 假时钟驱动的停滞与重连超时判定测试
//! + 客户端首帧 IDR 策略与丢帧事件单测
//! + 本机回环集成测试（真实 QUIC，见 `tests/loopback.rs`）
//! + 假实现集成测试（假 capture / encoder / transport 跑完整会话，
//!   见 `tests/fake_session.rs`）。

#![forbid(unsafe_code)]

pub mod client;
pub mod clock;
pub mod control;
pub mod error;
pub mod event;
pub mod handle;
pub mod host;
pub mod net_transport;
pub mod state;
pub mod summary;

pub use client::{
    ClientControlDeps, ClientDeps, ClientSession, ClientSessionConfig, PeerAddr,
    RECONNECT_RETRY_INTERVAL_MS,
};
pub use clock::{Clock, ManualClock, SystemClock};
pub use control::{
    caps_from_config, config_from_agreed, next_session_id, ControlContext, ControlOutcome,
    ControlRole, PairingConfig, STATS_INTERVAL, STATS_INTERVAL_MS,
};
pub use error::{message, SessionError};
pub use event::{
    ControlChannel, FrameConsumer, FramePipeline, InputSink, RateControl, ReconnectingInfo,
    SessionEvent, SessionInput, Transport, TransportFactory, VideoSender, FRAME_EXPIRED_CODE,
};
pub use handle::{SessionEventStream, SessionHandle, SessionTimeouts};
pub use host::{HostControlDeps, HostDeps, HostSession, HostSessionConfig};
pub use net_transport::{FrameChannelStats, FrameStatsHandle, NetTransport};
pub use state::{SessionState, StateMachine, ALL_STATES};
pub use summary::SessionSummary;
