//! # beam-render —— Beam 渲染层（Windows 客户端）
//!
//! 对应技术设计文档 **§3.10**（模块设计，与 beam-decode 共用一段）。
//!
//! ## 职责
//!
//! D3D11 flip-model 交换链上屏，支持可变刷新率；
//! 解码纹理直接交给交换链；叠加层由 Tauri 窗口负责。
//!
//! ## 边界
//!
//! - **负责**：上屏。
//! - **不负责**：不解码；不做 UI 叠加层。
//! - **依赖**：windows（D3D11）。
//! - **被依赖**：beam-client。
//!
//! ## 接口
//!
//! - [`Renderer`]：`present(&DecodedFrame)`。
//!
//! ## §4 分层说明
//!
//! `DecodedFrame` 定义在 L0 的 beam-proto，因此 beam-render 不必依赖 beam-decode。
//!
//! ## 线程模型
//!
//! 解码线程 → 单槽 → 渲染线程（按 vblank）。
//!
//! ## 错误码
//!
//! `E10xxx`：`10011` 渲染设备丢失、`10012` 交换链创建失败、`10013` 帧被丢弃；
//! 另有同段扩展 `10014`（后端未启用）。
//!
//! ## 性能
//!
//! 上屏等待平均半帧（§8）。

#![forbid(unsafe_code)]

pub mod error;
pub mod renderer;

pub use error::{message, RenderError};
pub use renderer::{D3D11Renderer, FramePacer, RenderConfig, Renderer};

/// 解码后的帧（定义在协议层，§4 分层说明）。
pub use beam_proto::DecodedFrame;
