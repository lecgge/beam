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
//!     on_frame(data: Vec<u8>/ByteBuffer, pts_us, capture_ts_us, key)
//!     on_event(SessionEvent)
//! }
//! ```
//!
//! ## 热路径：JNI DirectByteBuffer（§3.12 / §6）
//!
//! uniffi 走 `Vec<u8>` 会有一次拷贝，热路径（每帧 60–200 次）不划算。
//! 设计上：
//!
//! 1. Rust 侧维护一个 **DirectByteBuffer 池**（按帧大小分桶，复用已归还的缓冲）；
//! 2. `on_frame` 回调时把 `Bytes` 直接交给 JNI 层，JNI 层用
//!    `NewDirectByteBuffer` 指向 Rust 侧的内存，**不复制**；
//! 3. Kotlin 侧消费完（`queue()` 入解码队列）后把缓冲 **归还**给 Rust 池；
//! 4. 未归还的缓冲由池在超时后回收，避免 Kotlin 崩溃导致内存泄漏。
//!
//! 控制面（配对、协商、统计、错误）走 uniffi 常规路径，拷贝成本可忽略。
//!
//! 本轮先落地接口契约与回调桥接，DirectByteBuffer 池在 M1（MVP）随
//! Android `core-net` 一起实现；本文件中的设计说明即为实现依据。
//!
//! ## feature 门控
//!
//! `uniffi` feature 默认关闭，保证 crate 本身在任何环境都能 `cargo check` 通过。

#![forbid(unsafe_code)]

pub mod client;
pub mod error;

pub use client::{BeamClient, ClientConfig, DiscoveryListener, FrameSink, PeerInfo};
pub use error::{message, FfiError};
