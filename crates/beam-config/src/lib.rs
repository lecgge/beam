//! # beam-config —— Beam 配置与存储层
//!
//! 对应技术设计文档 **§3.14**（模块设计）。
//!
//! ## 职责
//!
//! 配置模型、校验、持久化（Windows `%APPDATA%/Beam/config.toml`、信任库 `trust.json`、
//! 私钥 DPAPI 加密）、按优先级合并各层配置。
//!
//! ## 边界
//!
//! - **负责**：配置模型、优先级合并、校验、文件读写、私钥保护抽象。
//! - **不负责**：UI、业务逻辑、网络传输。
//! - **依赖**：proto（共享类型）、serde / toml。
//! - **被依赖**：host、client、ffi、桌面端。
//!
//! ## 接口
//!
//! - [`merge`]：按 §3.14 优先级合并配置层。
//! - [`validate`]：校验最终配置。
//! - [`TomlFileStore`] / [`ConfigStore`]：持久化。
//! - [`SecretProtector`]：私钥加密抽象（Windows 上接 DPAPI）。
//!
//! ## 优先级（§3.14）
//!
//! 命令行 / 环境变量 > 会话内临时覆盖 > 用户配置文件 > 设备能力推导默认值 > 内置默认。
//!
//! ## 配置项
//!
//! - 视频（`[video]`）：`width` / `height` / `fps` / `bitrate` / `codec` / `encoder` / `hdr` /
//!   `slices`（§3.6）/ `sfe`（§3.6）；
//! - 抓屏（`[capture]`）：`backend`（§3.5 `auto|wgc|dxgi`）/ `display_id`（§3.5）/
//!   `min_update_interval`（§3.5）；
//! - 网络（`[network]`）、输入（`[input]`）、音频（`[audio]`）。
//!
//! 命令行形如 `--backend=wgc --slices=4 --sfe=true`，
//! 环境变量形如 `BEAM_BACKEND=wgc` / `BEAM_SLICES=4` / `BEAM_SFE=true`。
//!
//! ## 错误码
//!
//! `E11xxx`：`11001` 解析失败、`11002` 校验失败、`11003` 存储不可用。
//!
//! ## 测试
//!
//! 逐层优先级合并、解析 / 序列化往返、非法值校验。

#![forbid(unsafe_code)]

pub mod error;
pub mod model;
pub mod store;

pub use error::{message, ConfigError};
pub use model::{
    merge, validate, AudioMode, AudioOverrides, AudioSettings, CaptureBackend, CaptureOverrides,
    CaptureSettings, ConfigLayers, InputOverrides, InputSettings, NetworkOverrides,
    NetworkSettings, Overrides, ResolvedConfig, VideoOverrides, VideoSettings,
    MAX_MIN_UPDATE_INTERVAL_MS, MAX_SLICES,
};
pub use store::{
    default_config_path, default_trust_path, load_file_layer, parse_layers, save_file_layer,
    to_toml, ConfigStore, MemoryStore, PlaintextProtector, SecretProtector, TomlFileStore,
};
