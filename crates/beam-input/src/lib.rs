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
//! - [`InputPermissions`]：按设备（键 / 鼠 / 手柄）的注入权限。
//! - [`InputInjector`]：有界通道 + 专用注入线程（§3.8 线程模型）。
//!
//! ## 权限来源
//!
//! 本层**不认识**协议侧与配置侧的权限结构（§3.8 边界：只依赖 proto / windows /
//! vigem-client）。权限位由上层映射过来：
//!
//! - 协议侧 `beam_pair::TrustedPeer.perms`（`PeerPerms{view,input,audio,gamepad}`）；
//! - 配置侧 `beam_config::InputSettings{gamepad, allow_keyboard, allow_mouse}`。
//!
//! 映射后交给 [`SendInputSink::set_permissions`] 或 [`InputInjector::set_permissions`]。
//!
//! ## unsafe 策略（决策 D1）
//!
//! crate 根是 `#![deny(unsafe_code)]`，只在确实要调 Win32 裸 API 的地方局部
//! `#[allow(unsafe_code)]`（`SendInput` 调用与 `INPUT` union 字段读取，
//! 以及注入线程的 MMCSS / 线程优先级设置）。
//!
//! ## 线程模型
//!
//! 输入 stream 接收任务 → 有界通道 → 注入线程（高优先级）。
//!
//! 具体实现见 [`injector`]：接收任务只负责**入队**（不阻塞），
//! 真正的 `SendInput` 发生在专用 OS 线程 `beam-input-inject` 上，
//! 该线程优先用 Windows **MMCSS "Games"**，不可用时退到 `SetThreadPriority`
//! （退路会如实记录在 [`ThreadPriorityMode`] 里，不假装用了 MMCSS）。
//!
//! ## 错误码
//!
//! `E8xxx`：`8001` ViGEmBus 未安装、`8002` 权限拒绝；另有同段扩展 `8003`、`8004`。
//!
//! ## 性能
//!
//! 接收 → 注入 ≤1 ms。
//!
//! 延迟口径与统计接口见 [`injector`]：统计的是「**入队 → 注入完成**」，
//! 由 [`InjectorStats`] / [`LatencySnapshot`] 暴露（含直方图）。
//!
//! ## 里程碑
//!
//! MVP（键鼠）/ v1（手柄）。
//!
//! ## feature 门控
//!
//! `vigem` feature 默认开启（§7）；关闭后手柄挂载返回 `E8001`。

#![deny(unsafe_code)]

pub mod error;
pub mod injector;
pub mod sink;

pub use error::{message, InputError};
pub use injector::{
    InjectorStats, InputInjector, LatencySnapshot, ThreadPriorityMode, DEFAULT_QUEUE_CAPACITY,
    INJECTOR_THREAD_NAME, LATENCY_BUCKET_COUNT, LATENCY_BUCKET_US,
};
pub use sink::{
    GamepadHandle, GamepadHub, InputPermissions, InputSink, RumbleSink, SendInputSink,
    MAX_GAMEPAD_SLOTS,
};
