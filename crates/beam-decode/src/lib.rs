//! # beam-decode —— Beam 解码层（Windows 客户端）
//!
//! 对应技术设计文档 **§3.10**（模块设计，与 beam-render 共用一段）。
//!
//! ## 职责
//!
//! ffmpeg D3D11VA 硬解 HEVC / H.264（/AV1 v2）；解码纹理直接交给 D3D11 flip-model 交换链；
//! 支持可变刷新率；叠加层由 Tauri 窗口负责。
//!
//! ## 边界
//!
//! - **负责**：硬解码。
//! - **不负责**：不上屏（上屏是 beam-render 的职责）。
//! - **依赖**：ffmpeg-next（可选）。
//! - **被依赖**：beam-client。
//!
//! ## 接口
//!
//! - [`VideoDecoder`]：`decode(AssembledFrame) -> DecodedFrame`。
//!
//! ## §4 分层说明
//!
//! `AssembledFrame` 与 `DecodedFrame` 放在 L0 的 beam-proto，
//! 这样 beam-decode 与 beam-render 不必横向依赖（§4 规定 L1 之间只有 net→pair 可依赖）。
//!
//! ## 线程模型
//!
//! 解码线程 → 单槽 → 渲染线程（按 vblank）。
//!
//! ## 错误码
//!
//! `E10xxx`：`10001` 解码器不可用、`10002` 解码失败、`10003` 设备丢失；
//! 另有同段扩展 `10004`（后端未启用）。
//!
//! ## 性能
//!
//! 解码 T1 ≤8、T2 ≤5、T3 ≤4 ms。
//!
//! ## 里程碑
//!
//! MVP（简版）/ v1。

#![forbid(unsafe_code)]

pub mod decoder;
pub mod error;

pub use decoder::{make_decoded_frame, supports_codec, D3D11VaDecoder, DecodeConfig, VideoDecoder};
pub use error::{message, DecError};

/// 解码后的帧（定义在协议层，§4 分层说明）。
pub use beam_proto::DecodedFrame;
