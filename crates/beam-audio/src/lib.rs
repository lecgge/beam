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
//! - **依赖**：opus（可选）、windows（WASAPI）。
//! - **被依赖**：beam-host、beam-client、beam-ffi。
//!
//! ## 错误码
//!
//! `E7xxx`：`7001` 设备打开失败、`7002` Opus 编解码失败、`7003` 音频输出失败；
//! 另有同段扩展 `7004`（后端未启用）。
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
//! 真实 Opus 编解码依赖原生库，放在 `opus` feature 后面且默认关闭。

#![forbid(unsafe_code)]

pub mod audio;
pub mod error;

pub use audio::{
    frame_duration, AudioConfig, AudioMode, AudioOutput, AudioSource, OpusDecoder, OpusDecoderImpl,
    OpusEncoder, OpusEncoderImpl, PcmFrame, WasapiLoopbackSource, CAPTURE_TO_ENCODE_BUDGET_MS,
    DEFAULT_BITRATE_BPS, END_TO_END_BUDGET_MS, FRAME_DURATION_MS, SAMPLES_PER_FRAME, SAMPLE_RATE,
};
pub use error::{message, AudioError};
