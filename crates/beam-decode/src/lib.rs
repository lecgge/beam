//! # beam-decode —— Beam 解码层（Windows 客户端）
//!
//! 对应技术设计文档 **§3.10**（模块设计，与 beam-render 共用一段）。
//!
//! ## 职责
//!
//! ffmpeg D3D11VA 硬解 HEVC / H.264（/AV1 v2）；解码纹理直接交给 D3D11 flip-model 交换链；
//! 支持可变刷新率；叠加层由 Tauri 窗口负责。
//!
//! ## 边界
//!
//! - **负责**：硬解码。
//! - **不负责**：不上屏（上屏是 beam-render 的职责）。
//! - **依赖**：ffmpeg-next（可选）、windows。
//! - **被依赖**：beam-client。
//!
//! ## 接口
//!
//! - [`VideoDecoder`]：`decode(AssembledFrame) -> DecodedFrame`。
//!
//! ## §4 分层说明
//!
//! `AssembledFrame` 与 `DecodedFrame` 放在 L0 的 beam-proto，
//! 这样 beam-decode 与 beam-render 不必横向依赖（§4 规定 L1 之间只有 net→pair 可依赖）。
//!
//! ## 线程模型
//!
//! 解码线程 → 单槽 → 渲染线程（按 vblank）。
//!
//! 单槽实现在 [`slot::FrameSlot`]：解码线程发布最新一帧，渲染线程按 vblank 取走；
//! 渲染落后时旧帧被覆盖，槽位同时统计丢帧数。
//!
//! ## 跨 crate 设备契约（硬约束）
//!
//! D3D11 **不允许跨设备拷贝**。渲染层的 `D3D11Renderer::device()` 返回的是
//! **渲染层自己创建的 D3D11 设备**，交换链与后备缓冲都在它上面；
//! **解码纹理必须创建在同一个设备上**，否则既无法零拷贝，也会被 D3D11 拒绝。
//!
//! 因此本 crate **不自己 `CreateDevice`**：生产路径请用
//! [`D3D11VaDecoder::open_with_device`]，把渲染层的设备传进来。
//! §4 规定 L1 之间不能横向依赖，beam-decode 不能直接引用 beam-render 的类型，
//! 契约通过 `ID3D11Device` 这个平台类型来表达。
//! [`D3D11VaDecoder::open`]（自建设备）只给「单独跑解码器」的场景用。
//!
//! ## 错误码
//!
//! `E10xxx`：`10001` 解码器不可用、`10002` 解码失败、`10003` 设备丢失；
//! 另有同段扩展 `10004`（后端未启用，即本次构建没带 `ffmpeg` feature）。
//!
//! ## 性能
//!
//! 解码 T1 ≤8、T2 ≤5、T3 ≤4 ms。
//!
//! ## 性能埋点（§3.13）
//!
//! `perf-stats` feature **默认关闭**（§7：release 禁止开启）。开启后本层通过
//! 可选依赖 `beam-perf` 采集**「解码」阶段**的耗时（见 [`perf`] 模块），
//! 数据汇进 `beam_perf::Perf::snapshot()`；关闭时埋点**零开销**。
//!
//! ## 里程碑
//!
//! MVP（简版）/ v1。
//!
//! ## 验证状态
//!
//! - ✅ **「D3D11VA 硬解」已在本机真机验证**（§9 M-1 验证点③的解码侧）：
//!   见 `examples/d3d11va_decode_smoke.rs`——喂进 NVENC 真实产出的 HEVC 码流，
//!   用**注入的 D3D11 设备**解出 1920×1080 的画面，并核对渲染目标句柄。
//! - ⚠️ **「解码纹理上屏」不在本 crate 范围**（上屏是 beam-render 的职责）。
//!
//! 验证入口（都需要把 `$FFMPEG_DIR\bin` 加进 `PATH`——conda 的 ffmpeg 包只带自己的 DLL，
//! 第三方运行库 zlib / dav1d / opus / libxml2 … 是单独的 conda 包）：
//!
//! - `cargo run -p beam-decode --example d3d11va_decode_smoke --features ffmpeg`
//! - `cargo run -p beam-decode --example probe_decoders --features ffmpeg`

// 决策 D1：平台层局部放开 unsafe。crate 根用 `deny`，
// 真正需要 unsafe 的地方（FFmpeg 的 FFI、D3D11 的 COM 指针）逐个 `#[allow]` 并写明原因。
#![deny(unsafe_code)]

// 性能埋点宏（§3.13）：必须声明在使用它的模块之前（`#[macro_use]` 是文本作用域）。
#[macro_use]
mod perf;

pub mod decoder;
pub mod error;
pub mod slot;

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;

pub use decoder::{
    make_decoded_frame, supports_codec, D3D11VaDecoder, DecodeConfig, DecodedSurface, VideoDecoder,
};
pub use error::{message, DecError};
pub use slot::{DecodedOutput, FrameSlot};

/// 解码后的帧（定义在协议层，§4 分层说明）。
pub use beam_proto::DecodedFrame;
