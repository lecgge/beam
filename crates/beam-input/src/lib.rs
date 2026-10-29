//! # beam-input —— Beam 输入层（Windows）
//!
//! 对应技术设计文档 **§3.8**（模块设计）。
//!
//! ## 职责
//!
//! 输入事件模型在协议层定义（beam-proto）；主机侧 SendInput 注入键鼠、
//! vigem-client 虚拟 Xbox 手柄、震动回传；权限控制（按设备）。
//!
//! ## 边界
//!
//! - **负责**：主机侧输入注入与虚拟手柄。
//! - **不负责**：不采集客户端输入（Windows 客户端由 beam-client 采集，Android 由 `core-input`）。
//! - **依赖**：proto（事件模型）、windows、vigem-client。
//! - **被依赖**：beam-host。
//! - **禁止**：依赖 capture / encode（§3.8）。
//!
//! ## 接口
//!
//! - [`InputSink`]：`apply(InputEvent)`。
//! - [`GamepadHub`]：`attach(slot) -> GamepadHandle`。
//! - [`RumbleSink`]：震动回传。
//!
//! ## 线程模型
//!
//! 输入 stream 接收任务 → 有界通道 → 注入线程（高优先级）。
//!
//! ## 错误码
//!
//! `E8xxx`：`8001` ViGEmBus 未安装、`8002` 权限拒绝；另有同段扩展 `8003`。
//!
//! ## 性能
//!
//! 接收 → 注入 ≤1 ms。
//!
//! ## 里程碑
//!
//! MVP（键鼠）/ v1（手柄）。
//!
//! ## feature 门控
//!
//! `vigem` feature 默认开启（§7）；关闭后手柄挂载返回 `E8001`。

#![forbid(unsafe_code)]

pub mod error;
pub mod sink;

pub use error::{message, InputError};
pub use sink::{
    GamepadHandle, GamepadHub, InputSink, RumbleSink, SendInputSink, MAX_GAMEPAD_SLOTS,
};
