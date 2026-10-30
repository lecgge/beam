//! # beam-client —— Beam Windows 客户端编排层
//!
//! 对应技术设计文档 **§3.9**（会话编排）与 **§3.10**（decode / render）。
//!
//! ## 职责
//!
//! 把 decode / render 与会话层组装成 Windows 客户端；把网络收到的画面交给解码器，
//! 再把解码结果交给渲染器上屏。
//!
//! ## 边界
//!
//! - **负责**：依赖注入、适配器、编排。
//! - **不负责**：不实现平台细节。
//! - **依赖**：session、decode、render、net、discovery、config。
//! - **被依赖**：桌面端 Tauri（apps/desktop）。
//! - **允许**：引用平台 crate（§4）。
//!
//! ## 错误码
//!
//! 编排层只做组装，错误沿用 `E10xxx`（decode / render）与 `E2xxx`（net）。
//!
//! ## 通道（§5）
//!
//! - 控制 stream 0（双向可靠）：[`NetTransport`] 的 `ControlChannel` 实现；
//! - 输入 stream（单向可靠，客户端 → 主机）：`NetTransport::send_input`；
//! - 视频 / 音频：QUIC datagram；接收侧由 [`NetTransport`] 持久持有的
//!   `beam_net::VideoStream` 轮询取帧，过期帧按 §3.2 记为 **E2004**。
//!
//! 主机侧的对称实现（QUIC 接受端 / 主机侧控制循环）在 `beam-host`。
//!
//! ## 客户端侧音频（§3.7）
//!
//! - [`AudioReceiver`]：音频数据报 → Opus 解码 → WASAPI 输出；
//! - [`DemuxTransport`]：**按帧头 `type` 分流**的会话传输 ——
//!   音频数据报走音频支路，画面数据报走画面重组器，
//!   二者共用**同一条** QUIC 连接的 datagram 通道；
//! - 音频**不进 FEC 分组**、不分片（一帧 Opus ≈ 80 B ≪ 1184 B 负载上限），
//!   字节级格式由 `beam_audio::wire` 统一实现（主机与客户端共用同一份）；
//! - **音频失败不影响画面**：解码失败（`E7002`）、输出失败（`E7003`）
//!   都只让这一帧没播出来，画面照旧。
//!
//! ## 为什么音频分流住在 beam-client 而不是 beam-session
//!
//! 会话层下沉的 `NetTransport` 用 `beam_net::VideoStream` 收画面，而它会把
//! 连接上到达的**所有**数据报都读走 —— 音频数据报会被当成画面分片。
//! 「按 `type` 分流」必须发生在唯一的数据报读点上，
//! 而 `crates/beam-session/src/net_transport.rs` **不在本次改动的写入范围内**。
//! 因此本 crate 提供一个**可选的**传输实现 [`DemuxTransport`]：
//! 控制面 / 输入面**原样转发**给会话层那份传输，只有 datagram 读点换成自己的；
//! 画面重组与抖动缓冲仍用 `beam-net` 的同一套原语。
//! 需要在会话层长期落地时，见交付报告里的说明。

#![forbid(unsafe_code)]

pub mod adapters;
pub mod audio;
pub mod runtime;
pub mod transport;

pub use adapters::{
    DecodeRenderConsumer, FrameChannelStats, FrameStatsHandle, NetRateController, NetTransport,
    ReadyTransportFactory,
};
pub use audio::{
    default_audio_sink, AudioDatagramSink, AudioReceiveStats, AudioReceiveStatsHandle,
    AudioReceiver,
};
/// §3.7 的音频配置类型**转出**给上层（桌面端 L4 不能直接依赖 beam-audio L1，
/// 见 §4 的分层规则）。
pub use beam_audio::{AudioConfig, AudioMode};
pub use runtime::{ClientConfig, ClientError, ClientRuntime};
pub use transport::{DemuxTransport, DemuxTransportFactory};
