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
//! - [`D3D11Renderer`]：真实实现（`d3d11` feature）。
//! - [`FrameHandle`]：解码线程往单槽提交帧的句柄。
//!
//! ## §4 分层说明
//!
//! `DecodedFrame` 定义在 L0 的 beam-proto，因此 beam-render 不必依赖 beam-decode。
//!
//! ## 线程模型
//!
//! 解码线程 → 单槽 → 渲染线程（按 vblank）。
//!
//! - 解码线程：[`FrameHandle::submit`] 把解码帧覆盖式写进无锁单槽；
//! - 渲染线程：[`RenderThread`] 按节拍取走单槽里的最新帧，
//!   交给 [`D3D11Renderer::present`] 真正上屏（`Present` 与垂直刷新对齐）。
//!
//! ## 跨 crate 契约（重要）
//!
//! §3.10 要求「解码纹理直接交给 D3D11 flip-model 交换链」。
//! D3D11 **不允许跨设备拷贝**，因此解码纹理必须创建在渲染器使用的
//! **同一个 D3D11 设备**上——解码侧请复用 [`D3D11Renderer::device`]。
//! 解码纹理通过 `DecodedFrame::surface`（[`beam_proto::PlatformHandle`]）传递，
//! 渲染层只借用、不接管所有权。
//!
//! ## 错误码
//!
//! `E10xxx`：`10011` 渲染设备丢失、`10012` 交换链创建失败、`10013` 帧被丢弃；
//! 另有同段扩展 `10014`（后端未启用，**非文档定义的码位**）。
//!
//! ## 性能
//!
//! 上屏等待平均半帧（§8）。这是设计性质，**没有在本机实测**。
//!
//! ## unsafe 策略（决策 D1）
//!
//! crate 根是 `#![deny(unsafe_code)]`，只在确实需要裸指针 / COM 的地方
//! 局部 `#[allow(unsafe_code)]`：
//!
//! - `slot`：无锁单槽用原子指针交换堆指针；
//! - `d3d11`：D3D11 / DXGI 的 COM 调用（创建设备、交换链、渲染目标视图、呈现）。

#![deny(unsafe_code)]

pub mod error;
pub mod renderer;
mod slot;

#[cfg(feature = "d3d11")]
mod d3d11;

pub use error::{message, RenderError};
pub use renderer::{
    D3D11Renderer, FrameHandle, FramePacer, FrameStatistics, RenderConfig, RenderThread, Renderer,
    WindowHandle,
};

/// 解码后的帧（定义在协议层，§4 分层说明）。
pub use beam_proto::DecodedFrame;
