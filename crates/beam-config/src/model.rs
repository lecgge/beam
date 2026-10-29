//! 配置模型与优先级合并（§3.14）。
//!
//! §3.14 的优先级（高 → 低）：
//! 1. 命令行参数 / 环境变量
//! 2. 会话内临时覆盖
//! 3. 用户配置文件
//! 4. 设备能力推导默认值
//! 5. 内置默认
//!
//! 每个字段独立参与合并：高层没设置的字段不会覆盖低层。

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// 音频模式（§3.7 `audio_mode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    /// 主机采集 + 串流。
    HostAndStream,
    /// 只串流（主机不采集）。
    StreamOnly,
    /// 关闭。
    Off,
}

impl Default for AudioMode {
    fn default() -> Self {
        Self::Off
    }
}

/// 视频配置（各层覆盖）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoOverrides {
    /// 画面宽。
    pub width: Option<u16>,
    /// 画面高。
    pub height: Option<u16>,
    /// 目标帧率。
    pub fps: Option<u32>,
    /// 目标码率，bit/s。
    pub bitrate_bps: Option<u32>,
    /// 编解码器（`hevc` / `h264` / `av1`）。
    pub codec: Option<String>,
    /// 编码器（`nvenc` / `amf` / `qsv` / `auto`）。
    pub encoder: Option<String>,
    /// 是否开启 HDR。
    pub hdr: Option<bool>,
}

/// 网络配置（各层覆盖）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkOverrides {
    /// FEC 比例。
    pub fec_ratio: Option<f32>,
    /// 单个数据报上限。
    pub max_datagram: Option<usize>,
    /// 抖动缓冲帧数。
    pub jitter_frames: Option<u32>,
    /// 码率下限。
    pub bitrate_min: Option<u32>,
    /// 码率上限。
    pub bitrate_max: Option<u32>,
    /// 是否开启 mDNS 发现。
    pub discovery_enabled: Option<bool>,
    /// 网卡白名单。
    pub iface_allowlist: Option<Vec<String>>,
}

/// 输入配置（各层覆盖）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputOverrides {
    /// 是否启用手柄。
    pub gamepad: Option<bool>,
    /// 是否允许键盘输入。
    pub allow_keyboard: Option<bool>,
    /// 是否允许鼠标输入。
    pub allow_mouse: Option<bool>,
}

/// 音频配置（各层覆盖）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioOverrides {
    /// 音频模式。
    pub mode: Option<AudioMode>,
    /// 音频码率，bit/s。
    pub bitrate_bps: Option<u32>,
}

/// 一层配置覆盖。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overrides {
    /// 视频。
    pub video: VideoOverrides,
    /// 网络。
    pub network: NetworkOverrides,
    /// 输入。
    pub input: InputOverrides,
    /// 音频。
    pub audio: AudioOverrides,
}

impl Overrides {
    /// 用更高优先级层的值覆盖自身（仅覆盖自身为 `None` 的字段）。
    pub fn fill_from_lower(&mut self, lower: &Overrides) {
        merge_video(&mut self.video, &lower.video);
        merge_network(&mut self.network, &lower.network);
        merge_input(&mut self.input, &lower.input);
        merge_audio(&mut self.audio, &lower.audio);
    }

    /// 从命令行参数解析（`--width=1920` 形式）。
    pub fn from_args(args: &[String]) -> Result<Self, ConfigError> {
        let mut o = Self::default();
        for arg in args {
            let Some(body) = arg.strip_prefix("--") else {
                continue;
            };
            let (key, value) = body
                .split_once('=')
                .ok_or_else(|| ConfigError::ParseFailed {
                    reason: format!("命令行参数缺少 '=': {arg}"),
                })?;
            let parse_num = |v: &str| -> Result<u32, ConfigError> {
                v.parse().map_err(|_| ConfigError::ParseFailed {
                    reason: format!("{key} 的值不是合法数字: {v}"),
                })
            };
            match key {
                "width" => o.video.width = Some(parse_num(value)? as u16),
                "height" => o.video.height = Some(parse_num(value)? as u16),
                "fps" => o.video.fps = Some(parse_num(value)?),
                "bitrate" => o.video.bitrate_bps = Some(parse_num(value)?),
                "codec" => o.video.codec = Some(value.to_string()),
                "encoder" => o.video.encoder = Some(value.to_string()),
                "hdr" => o.video.hdr = Some(parse_bool(value)?),
                "fec-ratio" => {
                    o.network.fec_ratio =
                        Some(value.parse().map_err(|_| ConfigError::ParseFailed {
                            reason: format!("fec-ratio 的值不是合法数字: {value}"),
                        })?)
                }
                "max-datagram" => o.network.max_datagram = Some(parse_num(value)? as usize),
                "jitter-frames" => o.network.jitter_frames = Some(parse_num(value)?),
                "bitrate-min" => o.network.bitrate_min = Some(parse_num(value)?),
                "bitrate-max" => o.network.bitrate_max = Some(parse_num(value)?),
                "discovery" => o.network.discovery_enabled = Some(parse_bool(value)?),
                "gamepad" => o.input.gamepad = Some(parse_bool(value)?),
                "audio" => o.audio.mode = Some(parse_audio_mode(value)?),
                _ => {}
            }
        }
        Ok(o)
    }

    /// 从环境变量解析（`BEAM_*` 前缀）。
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut o = Self::default();
        if let Some(v) = env_var("BEAM_WIDTH") {
            o.video.width = Some(parse_u16("BEAM_WIDTH", &v)?);
        }
        if let Some(v) = env_var("BEAM_HEIGHT") {
            o.video.height = Some(parse_u16("BEAM_HEIGHT", &v)?);
        }
        if let Some(v) = env_var("BEAM_FPS") {
            o.video.fps = Some(parse_u32("BEAM_FPS", &v)?);
        }
        if let Some(v) = env_var("BEAM_BITRATE") {
            o.video.bitrate_bps = Some(parse_u32("BEAM_BITRATE", &v)?);
        }
        if let Some(v) = env_var("BEAM_CODEC") {
            o.video.codec = Some(v);
        }
        if let Some(v) = env_var("BEAM_ENCODER") {
            o.video.encoder = Some(v);
        }
        if let Some(v) = env_var("BEAM_FEC_RATIO") {
            o.network.fec_ratio = Some(v.parse().map_err(|_| ConfigError::ParseFailed {
                reason: format!("BEAM_FEC_RATIO 不是合法数字: {v}"),
            })?);
        }
        if let Some(v) = env_var("BEAM_GAMEPAD") {
            o.input.gamepad = Some(parse_bool(&v)?);
        }
        if let Some(v) = env_var("BEAM_AUDIO") {
            o.audio.mode = Some(parse_audio_mode(&v)?);
        }
        Ok(o)
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn parse_bool(v: &str) -> Result<bool, ConfigError> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::ParseFailed {
            reason: format!("不是合法布尔值: {v}"),
        }),
    }
}

fn parse_u32(key: &str, v: &str) -> Result<u32, ConfigError> {
    v.parse().map_err(|_| ConfigError::ParseFailed {
        reason: format!("{key} 不是合法数字: {v}"),
    })
}

fn parse_u16(key: &str, v: &str) -> Result<u16, ConfigError> {
    v.parse().map_err(|_| ConfigError::ParseFailed {
        reason: format!("{key} 不是合法数字: {v}"),
    })
}

fn parse_audio_mode(v: &str) -> Result<AudioMode, ConfigError> {
    match v.to_ascii_lowercase().as_str() {
        "host+stream" | "host_and_stream" => Ok(AudioMode::HostAndStream),
        "stream_only" | "streamonly" => Ok(AudioMode::StreamOnly),
        "off" => Ok(AudioMode::Off),
        _ => Err(ConfigError::ParseFailed {
            reason: format!("不是合法音频模式: {v}"),
        }),
    }
}

/// 最终生效的视频配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSettings {
    /// 画面宽。
    pub width: u16,
    /// 画面高。
    pub height: u16,
    /// 目标帧率。
    pub fps: u32,
    /// 目标码率，bit/s。
    pub bitrate_bps: u32,
    /// 编解码器。
    pub codec: String,
    /// 编码器。
    pub encoder: String,
    /// 是否开启 HDR。
    pub hdr: bool,
}

/// 最终生效的网络配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// FEC 比例。
    pub fec_ratio: f32,
    /// 单个数据报上限。
    pub max_datagram: usize,
    /// 抖动缓冲帧数。
    pub jitter_frames: u32,
    /// 码率下限。
    pub bitrate_min: u32,
    /// 码率上限。
    pub bitrate_max: u32,
    /// 是否开启 mDNS 发现。
    pub discovery_enabled: bool,
    /// 网卡白名单。
    pub iface_allowlist: Vec<String>,
}

/// 最终生效的输入配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSettings {
    /// 是否启用手柄。
    pub gamepad: bool,
    /// 是否允许键盘输入。
    pub allow_keyboard: bool,
    /// 是否允许鼠标输入。
    pub allow_mouse: bool,
}

/// 最终生效的音频配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSettings {
    /// 音频模式。
    pub mode: AudioMode,
    /// 音频码率，bit/s。
    pub bitrate_bps: u32,
}

/// 最终生效的完整配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedConfig {
    /// 视频。
    pub video: VideoSettings,
    /// 网络。
    pub network: NetworkSettings,
    /// 输入。
    pub input: InputSettings,
    /// 音频。
    pub audio: AudioSettings,
}

impl Default for ResolvedConfig {
    /// 内置默认（§3.14 优先级最低的一层）。
    fn default() -> Self {
        Self {
            video: VideoSettings {
                width: 1920,
                height: 1080,
                fps: 60,
                bitrate_bps: 20_000_000,
                codec: "hevc".to_string(),
                encoder: "auto".to_string(),
                hdr: false,
            },
            network: NetworkSettings {
                fec_ratio: 0.2,
                max_datagram: 1200,
                jitter_frames: 0,
                bitrate_min: 2_000_000,
                bitrate_max: 150_000_000,
                discovery_enabled: true,
                iface_allowlist: Vec::new(),
            },
            input: InputSettings {
                gamepad: false,
                allow_keyboard: true,
                allow_mouse: true,
            },
            audio: AudioSettings {
                mode: AudioMode::Off,
                bitrate_bps: 128_000,
            },
        }
    }
}

/// 全部配置层（§3.14）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigLayers {
    /// 命令行参数。
    pub cli: Overrides,
    /// 环境变量。
    pub env: Overrides,
    /// 会话内临时覆盖。
    pub session: Overrides,
    /// 用户配置文件。
    pub file: Overrides,
    /// 设备能力推导默认值。
    pub device: Overrides,
}

/// 按 §3.14 优先级合并各层。
///
/// 从最高优先级（命令行）向最低优先级（设备能力）依次填充，
/// 每个字段只被最高优先级的那一层决定；仍为空时回落到内置默认。
pub fn merge(layers: &ConfigLayers) -> ResolvedConfig {
    let builtin = ResolvedConfig::default();

    let mut video = VideoOverrides::default();
    let mut network = NetworkOverrides::default();
    let mut input = InputOverrides::default();
    let mut audio = AudioOverrides::default();

    for layer in [
        &layers.cli,
        &layers.env,
        &layers.session,
        &layers.file,
        &layers.device,
    ] {
        merge_video(&mut video, &layer.video);
        merge_network(&mut network, &layer.network);
        merge_input(&mut input, &layer.input);
        merge_audio(&mut audio, &layer.audio);
    }

    ResolvedConfig {
        video: VideoSettings {
            width: video.width.unwrap_or(builtin.video.width),
            height: video.height.unwrap_or(builtin.video.height),
            fps: video.fps.unwrap_or(builtin.video.fps),
            bitrate_bps: video.bitrate_bps.unwrap_or(builtin.video.bitrate_bps),
            codec: video.codec.unwrap_or(builtin.video.codec),
            encoder: video.encoder.unwrap_or(builtin.video.encoder),
            hdr: video.hdr.unwrap_or(builtin.video.hdr),
        },
        network: NetworkSettings {
            fec_ratio: network.fec_ratio.unwrap_or(builtin.network.fec_ratio),
            max_datagram: network.max_datagram.unwrap_or(builtin.network.max_datagram),
            jitter_frames: network
                .jitter_frames
                .unwrap_or(builtin.network.jitter_frames),
            bitrate_min: network.bitrate_min.unwrap_or(builtin.network.bitrate_min),
            bitrate_max: network.bitrate_max.unwrap_or(builtin.network.bitrate_max),
            discovery_enabled: network
                .discovery_enabled
                .unwrap_or(builtin.network.discovery_enabled),
            iface_allowlist: network
                .iface_allowlist
                .unwrap_or(builtin.network.iface_allowlist),
        },
        input: InputSettings {
            gamepad: input.gamepad.unwrap_or(builtin.input.gamepad),
            allow_keyboard: input.allow_keyboard.unwrap_or(builtin.input.allow_keyboard),
            allow_mouse: input.allow_mouse.unwrap_or(builtin.input.allow_mouse),
        },
        audio: AudioSettings {
            mode: audio.mode.unwrap_or(builtin.audio.mode),
            bitrate_bps: audio.bitrate_bps.unwrap_or(builtin.audio.bitrate_bps),
        },
    }
}

macro_rules! merge_impl {
    ($fn_name:ident, $target:ident, $source:ident, [$($field:ident),* $(,)?]) => {
        fn $fn_name(target: &mut $target, source: &$source) {
            $(
                if target.$field.is_none() {
                    target.$field = source.$field.clone();
                }
            )*
        }
    };
}

merge_impl!(
    merge_video,
    VideoOverrides,
    VideoOverrides,
    [width, height, fps, bitrate_bps, codec, encoder, hdr,]
);
merge_impl!(
    merge_network,
    NetworkOverrides,
    NetworkOverrides,
    [
        fec_ratio,
        max_datagram,
        jitter_frames,
        bitrate_min,
        bitrate_max,
        discovery_enabled,
        iface_allowlist,
    ]
);
merge_impl!(
    merge_input,
    InputOverrides,
    InputOverrides,
    [gamepad, allow_keyboard, allow_mouse]
);
merge_impl!(
    merge_audio,
    AudioOverrides,
    AudioOverrides,
    [mode, bitrate_bps]
);

/// 校验最终配置（§3.14）。
pub fn validate(cfg: &ResolvedConfig) -> Result<(), ConfigError> {
    if cfg.video.width < 320 || cfg.video.width > 7680 {
        return Err(ConfigError::ValidationFailed {
            reason: format!("宽度 {} 超出 320..7680", cfg.video.width),
        });
    }
    if cfg.video.height < 240 || cfg.video.height > 4320 {
        return Err(ConfigError::ValidationFailed {
            reason: format!("高度 {} 超出 240..4320", cfg.video.height),
        });
    }
    if cfg.video.fps == 0 || cfg.video.fps > 360 {
        return Err(ConfigError::ValidationFailed {
            reason: format!("帧率 {} 超出 1..360", cfg.video.fps),
        });
    }
    if cfg.network.bitrate_min > cfg.network.bitrate_max {
        return Err(ConfigError::ValidationFailed {
            reason: "码率下限大于上限".to_string(),
        });
    }
    if !(0.0..=1.0).contains(&cfg.network.fec_ratio) {
        return Err(ConfigError::ValidationFailed {
            reason: format!("FEC 比例 {} 超出 0..1", cfg.network.fec_ratio),
        });
    }
    if cfg.network.max_datagram < 1200 {
        return Err(ConfigError::ValidationFailed {
            reason: format!(
                "数据报上限 {} 小于 §3.2 要求的 1200",
                cfg.network.max_datagram
            ),
        });
    }
    if !matches!(cfg.video.codec.as_str(), "hevc" | "h264" | "av1") {
        return Err(ConfigError::ValidationFailed {
            reason: format!("未知编解码器: {}", cfg.video.codec),
        });
    }
    if !matches!(cfg.video.encoder.as_str(), "auto" | "nvenc" | "amf" | "qsv") {
        return Err(ConfigError::ValidationFailed {
            reason: format!("未知编码器: {}", cfg.video.encoder),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_defaults_are_used_when_nothing_set() {
        let cfg = merge(&ConfigLayers::default());
        assert_eq!(cfg, ResolvedConfig::default());
        validate(&cfg).expect("内置默认应当合法");
    }

    #[test]
    fn user_file_beats_device_default() {
        let layers = ConfigLayers {
            device: Overrides {
                video: VideoOverrides {
                    width: Some(1280),
                    ..Default::default()
                },
                ..Default::default()
            },
            file: Overrides {
                video: VideoOverrides {
                    width: Some(1920),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let cfg = merge(&layers);
        assert_eq!(cfg.video.width, 1920);
    }

    #[test]
    fn session_override_beats_user_file() {
        let layers = ConfigLayers {
            file: Overrides {
                video: VideoOverrides {
                    width: Some(1920),
                    fps: Some(60),
                    ..Default::default()
                },
                ..Default::default()
            },
            session: Overrides {
                video: VideoOverrides {
                    width: Some(2560),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let cfg = merge(&layers);
        assert_eq!(cfg.video.width, 2560);
        // 未被覆盖的字段仍来自配置文件。
        assert_eq!(cfg.video.fps, 60);
    }

    #[test]
    fn env_beats_session_and_cli_beats_env() {
        let layers = ConfigLayers {
            session: Overrides {
                video: VideoOverrides {
                    width: Some(1280),
                    ..Default::default()
                },
                ..Default::default()
            },
            env: Overrides {
                video: VideoOverrides {
                    width: Some(1600),
                    ..Default::default()
                },
                ..Default::default()
            },
            cli: Overrides {
                video: VideoOverrides {
                    width: Some(3840),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let cfg = merge(&layers);
        assert_eq!(cfg.video.width, 3840);

        let layers_no_cli = ConfigLayers {
            cli: Overrides::default(),
            ..layers
        };
        assert_eq!(merge(&layers_no_cli).video.width, 1600);
    }

    #[test]
    fn command_line_args_are_parsed() {
        let args: Vec<String> = [
            "--width=2560",
            "--fps=120",
            "--codec=h264",
            "--gamepad=true",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let o = Overrides::from_args(&args).expect("应当能解析");
        assert_eq!(o.video.width, Some(2560));
        assert_eq!(o.video.fps, Some(120));
        assert_eq!(o.video.codec.as_deref(), Some("h264"));
        assert_eq!(o.input.gamepad, Some(true));
    }

    #[test]
    fn bad_command_line_arg_is_parse_error() {
        let args = vec!["--width=abc".to_string()];
        let err = Overrides::from_args(&args).expect_err("非法数字必须失败");
        assert_eq!(err.code(), 11001);
    }

    #[test]
    fn validation_rejects_bad_values() {
        let mut cfg = ResolvedConfig::default();
        cfg.video.width = 10;
        let err = validate(&cfg).expect_err("非法宽度必须失败");
        assert_eq!(err.code(), 11002);

        let mut cfg = ResolvedConfig::default();
        cfg.network.max_datagram = 100;
        assert!(validate(&cfg).is_err());

        let mut cfg = ResolvedConfig::default();
        cfg.video.codec = "vp9".to_string();
        assert!(validate(&cfg).is_err());
    }
}
