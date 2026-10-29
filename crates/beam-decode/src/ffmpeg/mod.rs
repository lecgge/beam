//! ffmpeg 后端（`ffmpeg` feature 打开时才有）。
//!
//! 本模块是 §3.10「ffmpeg D3D11VA 硬解 HEVC / H.264（/AV1 v2）」的真实实现。
//!
//! **验证状态**：「D3D11VA 硬解」已在本机真机验证
//! （见 `examples/d3d11va_decode_smoke.rs`：真实 HEVC 码流 → 注入设备 → 解出 1920×1080）。

pub mod decoder;
pub mod hw;
pub mod probe;

pub use decoder::FfmpegD3D11VaDecoder;
pub use hw::HwDevice;
pub use probe::{probe_decoders, DecoderCaps};
