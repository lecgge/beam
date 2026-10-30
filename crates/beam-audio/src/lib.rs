//! # beam-audio —— Beam 音频层
//!
//! 对应技术设计文档 **§3.7**（模块设计）。
//!
//! ## 职责
//!
//! 主机 WASAPI loopback 48 kHz → Opus 5 ms 帧；
//! Windows 客户端 Opus 解码 + WASAPI 输出；Android 由 `core-audio` 解码。
//!
//! ## 边界
//!
//! - **负责**：采集、Opus 编解码、播放。
//! - **不负责**：不做混音 / 回声消除。
//! - **依赖**：ffmpeg-next（可选，Opus 编解码）、windows（WASAPI）。
//! - **被依赖**：beam-host、beam-client、beam-ffi。
//!
//! ## 已落地 / 未落地（诚实说明）
//!
//! - **已落地**：Opus 编解码（§3.7「Opus 5 ms 帧」）—— 用 ffmpeg 的 **libopus**
//!   编解码器，5 ms 帧（240 采样 @ 48 kHz）真机往返已通过单测（互相关 > 0.9）。
//!   打开 `opus` feature 才启用；关闭时返回 `E7004`（后端未启用）。
//! - **已落地**：**WASAPI loopback 采集**（主机侧，`WasapiLoopbackSource`）与
//!   **WASAPI 输出**（客户端侧，`WasapiAudioOutput`）—— 见 [`wasapi`] 模块。
//!   格式协商 / PCM 转换 / 分帧累积是**纯逻辑**，放在 [`pcm`] 模块里单测覆盖；
//!   真机路径（枚举设备、打开端点）默认**不跑**（`#[ignore]` + 环境变量门控）。
//! - **未落地**：Android 侧的解码与输出（由 `core-audio` 负责，不在本 crate）。
//!
//! ## 错误码
//!
//! `E7xxx`：`7001` 设备打开失败、`7002` Opus 编解码失败、`7003` 音频输出失败；
//! 另有同段扩展 `7004`（后端未启用）。
//! `7001`（枚举/激活设备失败）、`7002`（Opus 编解码失败）、`7003`（写缓冲失败）
//! 现在都有**真实产生路径**。
//!
//! ## 配置
//!
//! `audio_mode=host+stream|stream_only|off`、`bitrate=128k`。
//!
//! ## 性能
//!
//! 采集 → 编码 ≤6 ms，端到端目标 ≤40 ms；音画同步偏差 <30 ms。
//!
//! ## 里程碑
//!
//! v1。
//!
//! ## feature 门控
//!
//! 真实 Opus 编解码依赖原生库（ffmpeg 的原生 opus 编解码器，§7 的 LGPL 构建自带），
//! 放在 `opus` feature 后面且默认关闭。
//! **WASAPI 采集/输出不依赖该 feature** —— 它们是 Windows 平台原生后端，
//! 只有在非 Windows 平台上才返回 `E7004`。

#![deny(unsafe_code)]

pub mod audio;
pub mod error;
pub mod opus;
pub mod pcm;
pub mod wasapi;

pub use audio::{
    frame_duration, AudioConfig, AudioMode, AudioOutput, AudioSource, OpusDecoder, OpusDecoderImpl,
    OpusEncoder, OpusEncoderImpl, PcmFrame, WasapiAudioOutput, WasapiLoopbackSource,
    CAPTURE_TO_ENCODE_BUDGET_MS, DEFAULT_BITRATE_BPS, END_TO_END_BUDGET_MS, FRAME_DURATION_MS,
    SAMPLES_PER_FRAME, SAMPLE_RATE,
};
pub use error::{message, AudioError};
