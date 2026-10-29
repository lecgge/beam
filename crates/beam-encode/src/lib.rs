//! # beam-encode —— Beam 编码层（Windows）
//!
//! 对应技术设计文档 **§3.6**（模块设计）。
//!
//! ## 职责
//!
//! GPU 上 RGB→NV12（D3D11 VideoProcessor 或 compute shader）；
//! NVENC / AMF / QSV 硬编（ffmpeg-next 封装，D3D11 硬件帧上下文）；
//! 低延迟参数；切片输出；强制 IDR；码率热更新。
//!
//! ## 边界
//!
//! - **负责**：颜色转换与编码。
//! - **不负责**：不负责发包。
//! - **依赖**：ffmpeg-next（LGPL 动态库）、windows。
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
//! ## 线程模型
//!
//! 单独编码线程（与节拍线程合并），同步调用。
//!
//! ## 错误码
//!
//! `E6xxx`：`6001` 无可用硬编、`6002` 会话数超限、`6003` 参数不支持→降档；
//! 另有同段扩展 `6004`（后端未启用）。
//!
//! ## 配置
//!
//! `encoder=auto|nvenc|amf|qsv`、`codec=hevc|h264|av1`、`slices`、`sfe`。
//!
//! ## 性能
//!
//! 颜色转换 + 编码 T1 ≤7 ms（Ampere 级，Ada ≤4 ms）、T2 ≤3 ms、T3 ≤2 ms。
//!
//! ## feature 门控
//!
//! 真实硬编依赖原生 ffmpeg（LGPL 动态链接），因此放在 `ffmpeg` feature 后面且**默认关闭**；
//! 默认构建下 `probe()` 返回空列表、`encode()` 返回 `E6004`。

#![forbid(unsafe_code)]

pub mod encoder;
pub mod error;

pub use encoder::{
    probe, supports_codec, CapturedFrame, EncodeConfig, EncoderCaps, EncoderKind,
    HardwareVideoEncoder, Texture, VideoEncoder,
};
pub use error::{message, EncError};
