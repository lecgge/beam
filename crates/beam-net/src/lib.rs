//! # beam-net —— Beam 网络层
//!
//! 对应技术设计文档 **§3.2**（模块设计）与 **§5**（协议规格）。
//!
//! ## 职责
//!
//! quinn 端点；视频 / 音频 datagram 分包与重组；Reed-Solomon FEC；
//! 码率控制（基于丢包与 RTT 的 GCC 简化版）；接收端抖动缓冲；连接统计。
//!
//! ## 边界
//!
//! - **负责**：搬字节。分包、重组、FEC、码率决策。
//! - **不负责**：不懂编码格式；不做配对决策（TLS 校验回调由 pair 提供）。
//! - **依赖**：proto、pair（`PeerVerifier`）、quinn、reed-solomon-erasure。
//! - **被依赖**：session、ffi。
//! - **禁止**：依赖 capture / encode / decode。
//!
//! ## 接口
//!
//! - [`NetEndpoint::listen`] / [`NetEndpoint::connect`]：建端点。
//! - [`VideoSender`] / [`VideoReceiver`]：视频收发。
//! - [`RateController::on_feedback`]：码率决策。
//!
//! ## 数据结构
//!
//! [`EncodedFrame`] / [`AssembledFrame`] 定义在协议层（beam-proto），
//! 本 crate 通过 [`beam_proto`] 重导出，避免 §4 禁止的 L1 横向依赖。
//!
//! ## 线程模型
//!
//! tokio 多线程运行时；发送路径专用任务，UDP GSO 批量发包；
//! 接收重组单任务；FEC 解码在 rayon 或 blocking 线程（当前实现在接收任务内）。
//!
//! ## 错误码
//!
//! `E2xxx`（§3.2）：`2001` 连接超时、`2002` 证书拒绝、`2003` datagram 不支持、
//! `2004` 帧过期丢弃；另有同段扩展 `2005`–`2008`。
//!
//! ## 配置
//!
//! [`NetConfig`]：`fec_ratio`（默认 0.2，>150 Mbps 降到 0.1）、`max_datagram=1200`、
//! `jitter_frames`、`bitrate_min/max`。
//!
//! ## 性能
//!
//! 分包 + FEC 编码 T1 ≤0.8 ms、T2 ≤0.4 ms、T3 ≤0.3 ms；重组 + FEC 恢复同量级。
//!
//! ## 测试
//!
//! 丢包 / 乱序 / 抖动注入、FEC 恢复率、码率控制仿真。

#![forbid(unsafe_code)]

pub mod config;
pub mod datagram;
pub mod endpoint;
pub mod error;
pub mod fec;
pub mod rate;

pub use beam_proto::{AssembledFrame, EncodedFrame, Feedback};

/// 对端证书校验器（定义在配对层，§4 允许 net → pair）。
///
/// 这里重导出，使 client / ffi 不必直接依赖 beam-pair 也能注入校验器。
pub use beam_pair::PeerVerifier;

pub use config::{adaptive_fec_ratio, NetConfig, TlsIdentity};
pub use datagram::{packetize, EncodedFrameView, Packet, Reassembler};
pub use endpoint::{Conn, NetEndpoint, VideoReceiver, VideoSender, VideoStream};
pub use error::{message, NetError};
pub use fec::{parity_shard_count, FecCodec, FecGroup};
pub use rate::{BitrateDecision, RateAction, RateController};

/// ALPN 标识（§5）。
pub const ALPN: &[u8] = beam_proto::ALPN;
