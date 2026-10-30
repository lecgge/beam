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
//! 单槽是无锁的（原子指针交换），热路径不阻塞（§6）。
//!
//! ## 决策备注
//!
//! **D4（windows crate 版本分叉）**：本 crate 内部以别名引入 `windows 0.61.3`
//! （`windows-capture 1.5` 锁定的版本）做 COM 指针桥接，**对外暴露的 `Texture`
//! 仍用 `windows 0.62`**，以便与 `beam-encode` 的 D3D11 纹理同型、满足 §6
//! 「capture→encode 同一设备上零拷贝」。两版 D3D11 的 IID/vtable 由操作系统
//! 定义，ABI 相同，桥接安全。`windows-capture 2.x` 需要 rustc ≥ 1.88，与仓库
//! MSRV 1.86 冲突，故暂不升级。**未改变文档语义**（§2/§7 未规定版本号）。
//!
//! ## 错误码
//!
//! `E5xxx`：`5001` 无显示器、`5002` 独占全屏无法抓取→切 DXGI、`5003` 设备丢失。
//!
//! 另有 `E5004`（后端未启用）：**这不是文档 §3.5 定义的码位**，
//! 它只表示当前构建没有编译进真实抓屏后端（`wgc` feature 未开启）。
//! 真实抓屏路径的错误一律落到 5001 / 5002 / 5003 三个文档码位上。
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
//! - `MinUpdateInterval` 需要 Windows 11 24H2；
//! - Windows 10 22H2 没有这个能力，抓屏上限约 60fps——1080p60 恰好够但无余量，
//!   M1 验收须含 Windows 10 实测项；
//! - 主机显示器刷新率需 ≥ 目标帧率（§3.5 性能）。
//!
//! 关于「SDK 26100」：那句话约束的是 **C/C++ 编译**。
//! Rust 侧 `windows` 0.62 的绑定是预生成的，**已经包含**
//! `GraphicsCaptureSession::SetMinUpdateInterval`，编译期不依赖本机 SDK；
//! 真正决定这个能力有没有的是**运行时 Windows 版本**。
//! 本层的做法是运行时探测（`ApiInformation::IsPropertyPresent`），
//! 不支持时**明确降级**为系统默认更新间隔，并通过
//! [`CaptureDegradation::MinUpdateIntervalUnsupported`] 如实反映，
//! 不会假装设置成功。
//!
//! ## feature 门控
//!
//! 默认 feature 下提供可编译的类型与 trait 实现；真实 WGC / DXGI 调用在 `wgc` feature 后面，
//! 未开启时 `start` 返回 `E5004`，`latest` 恒为 `None`。
//!
//! ## 性能埋点（§3.13）
//!
//! `perf-stats` feature **默认关闭**（§7：release 禁止开启）。开启后本层通过
//! 可选依赖 `beam-perf` 采集**「抓屏」阶段**的耗时（见 [`perf`] 模块），
//! 数据汇进 `beam_perf::Perf::snapshot()`；关闭时埋点**零开销**。
//!
//! ## unsafe 策略（决策 D1）
//!
//! crate 根是 `#![deny(unsafe_code)]`，只在确实需要裸指针 / COM 的地方
//! 局部 `#[allow(unsafe_code)]`：
//!
//! - `slot`：无锁单槽用原子指针交换堆指针（所有权在生产/消费之间转移）；
//! - `wgc` / `dxgi`：D3D11 / DXGI 的 COM 调用与纹理拷贝。

#![deny(unsafe_code)]

// 性能埋点宏（§3.13）：必须声明在使用它的模块之前（`#[macro_use]` 是文本作用域）。
#[macro_use]
mod perf;

pub mod error;
mod slot;
pub mod source;

#[cfg(feature = "wgc")]
mod dxgi;
#[cfg(feature = "wgc")]
mod wgc;

pub use error::{message, CapError};
pub use source::{
    CaptureBackend, CaptureConfig, CaptureDegradation, CaptureSource, CapturedFrame, CursorMode,
    Texture, WgcCaptureSource,
};

/// 默认抓屏后端（§3.5 `backend=auto|wgc|dxgi`）。
pub const DEFAULT_BACKEND: CaptureBackend = CaptureBackend::Auto;
