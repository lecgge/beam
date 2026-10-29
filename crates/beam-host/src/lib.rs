//! # beam-host —— Beam 主机编排层
//!
//! 对应技术设计文档 **§3.9**（模块设计）与 **§4**（依赖与分层）。
//!
//! ## 职责
//!
//! 把平台 crate（capture / encode / audio / input / discovery）与
//! 会话层（beam-session）组装成一台可开播的 Windows 主机。
//!
//! ## 边界
//!
//! - **负责**：依赖注入、适配器、编排。
//! - **不负责**：不实现平台细节（那是 capture / encode / input 的职责）。
//! - **依赖**：session、capture、encode、audio、input、discovery、net、config、log。
//! - **被依赖**：桌面端 Tauri（apps/desktop）、beam-ffi 的主机侧。
//! - **允许**：引用平台 crate（§4 明确「平台 crate 只能被 host/client 引用」）。
//!
//! ## 适配器
//!
//! §3.9 要求 session 只依赖 trait，因此这里提供三个适配器：
//! - [`CaptureEncodePipeline`]：抓屏 → 编码 → `beam_session::FramePipeline`；
//! - [`NetVideoSender`]：网络发送 → `beam_session::VideoSender`；
//! - [`InputAdapter`]：输入注入 → `beam_session::InputSink`。
//!
//! ## 错误码
//!
//! 编排层自身只做组装，错误沿用各平台 crate 的 `E5xxx` / `E6xxx` / `E8xxx` / `E2xxx`。

#![forbid(unsafe_code)]

pub mod adapters;
pub mod runtime;

pub use adapters::{CaptureEncodePipeline, InputAdapter, NetVideoSender};
pub use runtime::{HostConfig, HostError, HostRuntime};
