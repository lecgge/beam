//! ffmpeg 后端（`ffmpeg` feature 打开时才有）。
//!
//! 本模块是 §3.6「NVENC/AMF/QSV 硬编（ffmpeg-next 封装，D3D11 硬件帧上下文）」
//! 的真实实现：
//!
//! - [`probe`]：真的去枚举并打开编码器，判定本机是否可用；
//! - [`encoder::FfmpegVideoEncoder`]：建立编码器上下文、送帧、出码流、切片、
//!   强制 IDR、码率热更新；
//! - [`hw`]：把本进程的 `ID3D11Device` 交给 ffmpeg，并建立 D3D11 硬件帧池。
//!
//! **验证状态**：本批没有送入任何真实帧，因此「NVENC 出码流」未经真实硬件验证。
//! 验证入口见 `examples/probe_encoders.rs`（能真的枚举并打开编码器）。

pub mod encoder;
pub mod hw;
pub mod probe;

pub use encoder::FfmpegVideoEncoder;
pub use probe::probe_encoders;
