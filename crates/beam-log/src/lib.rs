//! # beam-log —— Beam 日志层
//!
//! 对应技术设计文档 **§3.15**（模块设计）。
//!
//! ## 职责
//!
//! tracing + tracing-subscriber 初始化、JSON 文件轮转（7 天 / 50 MB）、
//! 统一日志字段、敏感信息脱敏。
//!
//! ## 边界
//!
//! - **负责**：日志初始化、轮转策略、字段规范、脱敏。
//! - **不负责**：业务逻辑、UI 展示。
//! - **依赖**：tracing、tracing-subscriber。
//! - **被依赖**：host、client、ffi、各平台 crate。
//!
//! ## 接口
//!
//! - [`init`]：初始化全局日志（可重复调用）。
//! - [`LogFields`]：标准字段 `session_id` / `peer` / `stage` / `code`。
//! - [`Sensitive`]：敏感值包装，`Debug` / `Display` 只输出掩码。
//!
//! ## 字段规范
//!
//! `session_id`、`peer`、`stage`、`code`（§3.15）。
//!
//! ## 禁止事项
//!
//! 禁止记录 PIN、私钥、token：一律用 [`crate::redact`] 里的函数或 [`Sensitive`] 包装。
//!
//! ## 热路径
//!
//! 热路径只允许 `trace!`，且默认级别不包含 `trace`（§3.15）。
//!
//! ## 错误码
//!
//! `E12xxx`：`12001` 初始化失败。

#![forbid(unsafe_code)]

pub mod error;
pub mod fields;
pub mod init;
pub mod redact;
pub mod rolling;

pub use error::{message, LogError};
pub use fields::LogFields;
pub use init::{default_log_dir, init, LogConfig, Logger};
pub use redact::{redact_fingerprint, redact_pin, redact_secret, redact_token, Sensitive};
pub use rolling::{RollingFile, DEFAULT_MAX_BYTES, DEFAULT_MAX_DAYS, DEFAULT_MAX_FILES};
