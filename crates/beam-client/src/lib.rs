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

#![forbid(unsafe_code)]

pub mod adapters;
pub mod runtime;

pub use adapters::{DecodeRenderConsumer, NetTransport};
pub use runtime::{ClientConfig, ClientError, ClientRuntime};
