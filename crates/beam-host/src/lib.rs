//! # beam-host —— Beam 主机编排层
//!
//! 对应技术设计文档 **§3.9**（模块设计）与 **§4**（依赖与分层）。
//!
//! ## 职责
//!
//! 把平台 crate（capture / encode / audio / input / discovery）与
//! 会话层（beam-session）组装成一台可开播的 Windows 主机。
//!
//! ## 边界
//!
//! - **负责**：依赖注入、适配器、编排。
//! - **不负责**：不实现平台细节（那是 capture / encode / input 的职责）。
//! - **依赖**：session、capture、encode、audio、input、discovery、net、config、log。
//! - **被依赖**：桌面端 Tauri（apps/desktop）、beam-ffi 的主机侧。
//! - **允许**：引用平台 crate（§4 明确「平台 crate 只能被 host/client 引用」）。
//!
//! ## 适配器
//!
//! §3.9 要求 session 只依赖 trait，因此这里提供三个适配器：
//! - [`CaptureEncodePipeline`]：抓屏 → 编码 → `beam_session::FramePipeline`；
//! - [`NetVideoSender`]：网络发送 → `beam_session::VideoSender`；
//! - [`InputAdapter`]：输入注入 → `beam_session::InputSink`。
//!
//! ## 主机侧输入接线（§3.9 / §3.8）
//!
//! - [`map_input_permissions`]：把**协议侧**对端权限（`beam_pair::TrustedPeer.perms`）
//!   与**配置侧**输入设置（`beam_config::InputSettings`）取**交集**，
//!   映射成按设备的注入权限（`beam_input::InputPermissions`）；
//! - 输入泵（[`crate::runtime::HostRuntime::start_quic_session_with_pipeline`]）在调用方
//!   给出**占位**注入器（[`NoopInputSink`]，桌面端受 §4 限制拿不到平台 crate）时，
//!   换成**真实注入器**（`beam_input::InputInjector`：有界通道 + 专用注入线程 + MMCSS）。
//!
//! ## 主机侧 QUIC 接受端（§5）
//!
//! [`HostTransport`] 与 [`HostInput`] 是主机侧的 QUIC 适配器：
//! 接受控制双向流（§5 控制 stream 0）、接收输入单向流（§5 输入 stream），
//! 画面走 QUIC datagram。它们原本寄居在 `beam-client`（职责倒挂），
//! 本 crate 才是它们的 rightful 归属。
//!
//! ## 平台会话与监听编排（§3.9）
//!
//! - [`HostListener`]：QUIC 监听（§3.2），把「监听 → 接受连接 → 起会话」串起来；
//! - [`HostRuntime::start_platform_session`]（`platform` feature）：
//!   在本 crate **内部**构造 WGC 抓屏源（§3.5）与 NVENC 编码器（§3.6），
//!   交给 [`HostRuntime::start_quic_session`]。
//!
//! `platform` feature 默认关闭；打开它会把 `beam-capture` 的 `wgc`
//! 与 `beam-encode` 的 `ffmpeg` 两个后端传递进来。
//! 桌面端因此**不需要**直接依赖平台 crate（§4）。
//!
//! ## 主机侧配对的安全环节（§3.3 / 产品方案 §8）
//!
//! [`HostTrustGate`] 是主机侧的信任闸门：未写入信任库的对端**一律拒绝握手**
//! （但指纹被记住），用户人工确认后写库，下一次握手才放行。
//! 这是产品方案第 102 行「主机端必须弹窗人工确认后才会写入信任库」
//! 在代码里的落点，**不可省略**。
//!
//! ## 决策备注
//!
//! **D5（跨 crate 的 D3D11 设备契约）**：设备由**本 crate（串流编排层，L3）**
//! 统一创建，再注入 capture / encode / decode / render。平台 crate 之间**不做**
//! 横向依赖（§4 规定 L1 之间仅 net→pair 可例外），因此契约用平台类型
//! `ID3D11Device` 表达：解码侧入口是 `beam_decode::D3D11VaDecoder::open_with_device`，
//! 渲染侧是 `beam_render::D3D11Renderer::device()`，后者已做运行期设备校验
//! （不匹配则该帧清屏并告警，**不静默拷错**）。**未改变文档语义**。
//!
//! ## 主机侧音频接线（§3.7）
//!
//! - [`AudioPipeline`]：WASAPI loopback 采集 → Opus 5 ms 帧 → **音频数据报**；
//! - [`NetAudioSender`]：音频与画面走**同一条** QUIC 连接的 datagram 通道，
//!   音频帧头的 `type = 1`（§5）；
//! - 音频**不进 FEC 分组**、**不分片**（一帧 Opus ≈ 80 B ≪ 1184 B 的负载上限），
//!   字节级格式由 `beam_audio::wire` 统一实现（主机与客户端共用同一份）；
//! - 启停跟随 §3.7 的 `audio_mode`：`host+stream` 采集并串流、`stream_only`
//!   主机不采集、`off` 完全不接；
//! - **音频失败不影响画面**：设备打不开（`E7001`）、Opus 编解码失败（`E7002`）、
//!   发送失败（`E2xxx`）都只停掉音频链路，串流继续。
//!
//! ## 错误码
//!
//! 编排层自身只做组装，错误沿用各平台 crate 的 `E5xxx` / `E6xxx` / `E8xxx` / `E2xxx`。
//! 音频链路的错误沿用 `beam-audio` 的 `E7xxx`（见 [`AudioPipelineError::code`]）。

#![forbid(unsafe_code)]

pub mod adapters;
pub mod audio;
pub mod input;
pub mod net;
pub mod pairing;
/// 平台会话工厂与监听编排（需要 `platform` 或 `platform-wgc` feature：
/// WGC 抓屏 + NVENC 硬编）。
#[cfg(any(feature = "platform", feature = "platform-wgc"))]
pub mod platform;
pub mod runtime;

pub use adapters::{
    CaptureEncodePipeline, EmptyPipeline, InputAdapter, NetVideoSender, NoopInputSink,
};
pub use audio::{
    AudioDatagramSender, AudioPipeline, AudioPipelineError, AudioStats, AudioStatsHandle,
    NetAudioSender,
};
pub use input::map_input_permissions;
pub use net::{HostInput, HostListener, HostTransport};
pub use pairing::{HostTrustGate, ObservedPeer};
pub use runtime::{default_frame_pipeline, HostConfig, HostError, HostRuntime, HostSessionHandle};
