//! # beam-ffi —— Beam FFI 层（uniffi 导出给 Android）
//!
//! 对应技术设计文档 **§3.12**（Android 模块）。
//!
//! ## 职责
//!
//! 导出 `BeamClient`（`new` / `browse` / `pair_qr` / `pair_pin` / `connect`）
//! 与 `FrameSink` 回调接口给 Android `core-net` 使用。
//!
//! ## 边界
//!
//! - **负责**：FFI 导出面、帧回调桥接、事件桥接。
//! - **不负责**：解码 / 渲染 / 输入采集（留在 Kotlin 侧，§3.12）。
//! - **依赖**：session、net、pair、discovery。
//! - **被依赖**：Android `core-net`。
//! - **禁止**：不得引入 capture / encode / input / decode / render 等平台 crate（§4）。
//!
//! ## 导出面（§3.12）
//!
//! ```text
//! BeamClient::new(config)
//! BeamClient::browse(listener)
//! BeamClient::pair_qr(payload)
//! BeamClient::pair_pin(peer, pin)
//! BeamClient::connect(peer, cfg, sink: FrameSink)
//!
//! callback interface FrameSink {
//!     on_frame(data: Vec<u8>, pts_us, capture_ts_us, key)
//!     on_event(SessionEvent)
//! }
//! ```
//!
//! 具体类型与签名见 [`ffi`] 模块（仅在 `uniffi` feature 打开时编译）。
//!
//! ## 热路径：JNI DirectByteBuffer —— **尚未实现，当前是拷贝路径**
//!
//! §3.12 要求「热路径用 JNI 直接 ByteBuffer(DirectByteBuffer) 避免 uniffi 拷贝」。
//! **本仓库当前没有实现这一点**：帧数据经 uniffi 的 `Vec<u8>` 传递，
//! 每帧一次内存拷贝（60–200 fps 下不可忽略）。
//!
//! 未实现的原因与代价：
//!
//! - Rust 侧**没有** `jni` 依赖，也没有 `libbeam_ffi.so` 的 JNI 导出符号；
//! - crate 级 `#![forbid(unsafe_code)]` 与 JNI 零拷贝所需的 `unsafe` 冲突
//!   （开启 `uniffi` feature 后只能放宽为 `allow`，见本文件末尾的 lint 说明）。
//!
//! 要补齐需要三件事：
//!
//! 1. 引入 `jni` crate，在 `libbeam_ffi.so` 里导出 JNI 函数，
//!    用 `NewDirectByteBuffer` 指向 Rust 侧内存（**不复制**）；
//! 2. Rust 侧维护 **DirectByteBuffer 池**（按帧大小分桶，复用已归还的缓冲）；
//! 3. Kotlin 侧消费完（`queue()` 入解码队列）后把缓冲 **归还**给 Rust 池，
//!    未归还的缓冲由池在超时后回收，避免 Kotlin 崩溃导致内存泄漏。
//!
//! 控制面（配对、协商、统计、错误）走 uniffi 常规路径，拷贝成本可忽略。
//!
//! **在补齐之前，不要把本 crate 的帧通路当成零拷贝**；相关差距条目见
//! `docs/beam-gap-list.md` 的 F-07 / P6-06。
//!
//! ## feature 门控
//!
//! `uniffi` feature 默认关闭，保证 crate 本身在任何环境都能 `cargo check` 通过：
//!
//! - 关闭时：只编译 `client` / `error` 两个模块，导出面不参与编译；
//! - 打开时：额外编译 [`ffi`] 模块并注册 uniffi scaffolding，
//!   产物为 `cdylib`（`libbeam_ffi.so`），供 Android `System.loadLibrary("beam_ffi")` 加载。

// `uniffi` 生成的 scaffolding 含 `unsafe impl`（FfiConverter / Lift / Lower），
// 与 crate 级的 `forbid(unsafe_code)` 冲突。因此：
// - feature 关闭时保持 `forbid`（不牺牲任何安全性）；
// - feature 打开时放宽为 `allow`，业务模块各自 `#![deny(unsafe_code)]` 兜底。
#![cfg_attr(not(feature = "uniffi"), forbid(unsafe_code))]
#![cfg_attr(feature = "uniffi", allow(unsafe_code))]

pub mod client;
pub mod error;

#[cfg(feature = "uniffi")]
pub mod ffi;

pub use client::{BeamClient, ClientConfig, DiscoveryListener, FrameSink, PeerInfo};
pub use error::{message, FfiError};

// uniffi scaffolding 注册（proc-macro 模式）。
//
// 生成 `UniFfiTag`、RustBuffer 桥接函数与 namespace 元数据，
// 供 `uniffi-bindgen generate --library … --language kotlin` 提取绑定信息。
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();
