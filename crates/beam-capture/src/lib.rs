//! # beam-capture —— Beam 抓屏层（Windows）
//!
//! 对应技术设计文档 **§3.5**（模块设计）。
//!
//! ## 职责
//!
//! WGC（主）/ DXGI 复制（回退）抓屏，输出 GPU 上的 D3D11 纹理；设置 `MinUpdateInterval`；
//! 光标单独通道。
//!
//! ## 边界
//!
//! - **负责**：抓屏、纹理交付、帧节拍。
//! - **不负责**：不做颜色转换和编码；不拷到 CPU。
//! - **依赖**：windows-capture、windows crate、beam-config（配置模型）。
//! - **被依赖**：beam-host。
//! - **禁止**：依赖 net（§3.5）。
//!
//! ## 接口
//!
//! - [`CaptureSource`]：`start` / `latest`（非阻塞取最新帧）/ `stop`。
//! - [`CapturedFrame`]：纹理 + 时间戳 + 是否变化。
//!
//! ## 线程模型
//!
//! WGC 回调线程只更新「最新帧」槽（单槽交换）；编码节拍线程按目标帧率拉取。
//!
//! ## 错误码
//!
//! `E5xxx`：`5001` 无显示器、`5002` 独占全屏无法抓取→切 DXGI、`5003` 设备丢失；
//! 另有同段扩展 `5004`（后端未启用）。
//!
//! ## 配置
//!
//! `backend=auto|wgc|dxgi`、`display_id`、`min_update_interval`。
//!
//! **单一真值来源**：这三项由 beam-config 承载（§3.14 的模型、校验与持久化），
//! 本 crate 用 `CaptureConfig::from(&ResolvedConfig)` 派生，不再自带一份配置定义；
//! [`CaptureBackend`] 同样是 beam-config 的枚举，本 crate 只做重新导出。
//!
//! ## 平台限制（§3.5 末尾）
//!
//! - `MinUpdateInterval` 需要 SDK 26100（Windows 11 24H2）；
//! - Windows 10 22H2 没有这个能力，MVP 阶段又无 DXGI 回退，
//!   **抓屏上限约 60fps**——1080p60 恰好够但无余量，M1 验收须含 Windows 10 实测项；
//! - 主机显示器刷新率需 ≥ 目标帧率（§3.5 性能）。
//!
//! ## feature 门控
//!
//! 默认 feature 下提供可编译的类型与 trait 实现；真实 WGC 调用在 `wgc` feature 后面，
//! 未开启时 `start` 返回 `E5004`，`latest` 恒为 `None`。

#![forbid(unsafe_code)]

pub mod error;
pub mod source;

pub use error::{message, CapError};
pub use source::{
    CaptureBackend, CaptureConfig, CaptureSource, CapturedFrame, Texture, WgcCaptureSource,
};

/// 默认抓屏后端（§3.5 `backend=auto|wgc|dxgi`）。
pub const DEFAULT_BACKEND: CaptureBackend = CaptureBackend::Auto;
