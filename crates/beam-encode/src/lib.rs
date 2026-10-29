//! # beam-encode —— Beam 编码层（Windows）
//!
//! 对应技术设计文档 **§3.6**（模块设计）。
//!
//! ## 职责
//!
//! GPU 上 RGB→NV12（D3D11 VideoProcessor）；
//! NVENC / AMF / QSV 硬编（ffmpeg-next 封装，D3D11 硬件帧上下文）；
//! 低延迟参数；切片输出；强制 IDR；码率热更新。
//!
//! ## 边界
//!
//! - **负责**：颜色转换与编码。
//! - **不负责**：不负责发包。
//! - **依赖**：ffmpeg-next（LGPL 动态库）、windows、beam-config（配置模型）。
//! - **被依赖**：beam-host。
//! - **禁止**：依赖 net / session（§3.6）。
//!
//! ## 接口
//!
//! - [`VideoEncoder`]：`encode` / `request_idr` / `set_bitrate`。
//! - [`probe`]：探测本机可用编码器。
//!
//! ## 参数基线（§3.6）
//!
//! NVENC P1 + ultra-low-latency、CBR、无 B 帧、无 lookahead、VBV = 1 帧、intra-refresh；
//! Ada 上 T1 及高帧率档开启 SFE；AMF `ultralowlatency` + `vbr_latency`；QSV `veryfast`。
//!
//! 参数到 ffmpeg 选项的翻译在 [`params`]，并由该模块的单元测试逐条锁定。
//!
//! ## 线程模型
//!
//! 单独编码线程（与节拍线程合并），同步调用。
//!
//! ## 错误码
//!
//! `E6xxx`：`6001` 无可用硬编、`6002` 会话数超限、`6003` 参数不支持→降档；
//! 另有同段扩展 `6004`（后端未启用，即本次构建没带 `ffmpeg` feature）。
//!
//! ## 配置
//!
//! `encoder=auto|nvenc|amf|qsv`、`codec=hevc|h264|av1`、`slices`、`sfe`。
//!
//! **单一真值来源**：这些配置项由 beam-config 承载（§3.14 的模型、校验与持久化），
//! 本 crate 用 `EncodeConfig::from(&ResolvedConfig)` 派生，不再自带一份配置定义。
//!
//! ## 性能
//!
//! 颜色转换 + 编码 T1 ≤7 ms（Ampere 级，Ada ≤4 ms）、T2 ≤3 ms、T3 ≤2 ms。
//!
//! ## feature 门控
//!
//! 真实硬编依赖原生 ffmpeg（LGPL 动态链接），因此放在 `ffmpeg` feature 后面且**默认关闭**；
//! 默认构建下 `probe()` 返回空列表、`encode()` 返回 `E6004`。
//!
//! ## 验证状态
//!
//! - ✅ **「NVENC 出码流」已在本机真机验证**（§9 M-1 验证点②）：
//!   见 `examples/nvenc_encode_smoke.rs`——真的用 NVENC 编出 HEVC 码流，
//!   再用软件解码器把码流解回来核对分辨率。
//! - ⚠️ **「GPU 上 RGB→NV12」未验证**：本机 D3D11 VideoProcessor 的
//!   `CreateVideoProcessorInputView` / `CreateVideoProcessorOutputView` 返回
//!   `E_INVALIDARG(0x80070057)`，VideoProcessor 这条转换路走不通；
//!   已跑通的是「NV12 纹理 → D3D11 硬件帧 → NVENC」。
//! - ⚠️ 真实**抓屏纹理**接入（WGC 纹理 → 编码器）尚未联调：本批的验证入口自造纹理。
//!
//! 验证入口（都需要把 `$FFMPEG_DIR\bin` 加进 `PATH`——conda 的 ffmpeg 包只带自己的 DLL，
//! 第三方运行库 zlib / dav1d / opus / libxml2 … 是单独的 conda 包）：
//!
//! - `cargo run -p beam-encode --example nvenc_encode_smoke --features ffmpeg`
//! - `cargo run -p beam-encode --example probe_encoders --features ffmpeg`

// 决策 D1：平台层局部放开 unsafe。crate 根用 `deny`，
// 真正需要 unsafe 的地方（FFmpeg 的 FFI、D3D11 的 COM 指针）逐个 `#[allow]` 并写明原因。
#![deny(unsafe_code)]

pub mod encoder;
pub mod error;
pub mod params;

#[cfg(all(windows, feature = "ffmpeg"))]
pub mod color;
#[cfg(all(windows, feature = "ffmpeg"))]
pub mod ffmpeg;

pub use encoder::{
    probe, select_encoder_kind, supports_codec, CapturedFrame, EncodeConfig, EncoderCaps,
    EncoderKind, HardwareVideoEncoder, Texture, VideoEncoder,
};
pub use error::{message, EncError};
