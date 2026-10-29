//! 能力集、协议版本与协商（§3.1 `Caps`、§5 版本协商）。

use serde::{Deserialize, Serialize};

use crate::error::ProtoError;

/// 协议主版本号（§5：主版本不同直接拒绝 `E1001`）。
pub const PROTO_MAJOR: u8 = 1;

/// 协议次版本号（§5：次版本向后兼容，未知字段忽略）。
pub const PROTO_MINOR: u8 = 0;

/// 编解码器类型（§3.1 `Caps.codecs`）。
///
/// 变体顺序即线上编码顺序，不得随意调整。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CodecKind {
    /// H.264 / AVC。
    H264,
    /// HEVC / H.265。
    Hevc,
    /// AV1（v2，§7 `av1` feature）。
    Av1,
}

impl CodecKind {
    /// 稳定的英文名，用于日志与配置。
    pub const fn as_str(self) -> &'static str {
        match self {
            CodecKind::H264 => "h264",
            CodecKind::Hevc => "hevc",
            CodecKind::Av1 => "av1",
        }
    }

    /// 由线上序号还原（postcard 用变体序号编码枚举）。
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            0 => CodecKind::H264,
            1 => CodecKind::Hevc,
            _ => CodecKind::Av1,
        }
    }
}

/// 分辨率（§3.1 `Caps.max_res`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// 宽。
    pub width: u16,
    /// 高。
    pub height: u16,
}

impl Resolution {
    /// 构造分辨率。
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

impl Default for Resolution {
    /// 1080p（§8 T0 档位基线）。
    fn default() -> Self {
        Self::new(1920, 1080)
    }
}

/// 协议版本（§5 `Hello` 中的 `proto_ver`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// 主版本：不同即拒绝。
    pub major: u8,
    /// 次版本：向后兼容，取双方较小值。
    pub minor: u8,
}

impl ProtocolVersion {
    /// 本地当前协议版本。
    pub const CURRENT: Self = Self {
        major: PROTO_MAJOR,
        minor: PROTO_MINOR,
    };

    /// 主版本是否相同（§5）。
    pub const fn same_major(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

/// 能力位图（§3.1 `Caps.features`，§5 `features` 位图）。
pub mod feature {
    /// 支持音频（v1）。
    pub const AUDIO: u64 = 1 << 0;
    /// 支持手柄（v1）。
    pub const GAMEPAD: u64 = 1 << 1;
    /// 支持虚拟显示器（v1）。
    pub const VIRTUAL_DISPLAY: u64 = 1 << 2;
    /// 支持 FEC（v1）。
    pub const FEC: u64 = 1 << 3;
    /// 支持 HDR（v2）。
    pub const HDR: u64 = 1 << 4;
}

/// 能力集（§3.1 `Caps{codecs,max_res,max_fps,features}`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// 支持的编解码器。
    pub codecs: Vec<CodecKind>,
    /// 支持的最大分辨率。
    pub max_res: Resolution,
    /// 支持的最大帧率。
    pub max_fps: u32,
    /// 特性位图，见 [`feature`]。
    pub features: u64,
}

impl Caps {
    /// 是否具备某个特性位。
    pub const fn has_feature(&self, bit: u64) -> bool {
        self.features & bit != 0
    }

    /// 内置默认能力（1080p60 / H.264 + HEVC / 音频）。
    pub fn default_host() -> Self {
        Self {
            codecs: vec![CodecKind::H264, CodecKind::Hevc],
            max_res: Resolution::default(),
            max_fps: 60,
            features: feature::AUDIO,
        }
    }
}

impl Default for Caps {
    fn default() -> Self {
        Self::default_host()
    }
}

/// 协商结果（§3.1 `negotiate` 的返回值）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreed {
    /// 双方共同支持的编解码器（取较高者）。
    pub codec: CodecKind,
    /// 协商宽度。
    pub width: u16,
    /// 协商高度。
    pub height: u16,
    /// 协商帧率。
    pub fps: u32,
    /// 双方特性交集。
    pub features: u64,
}

/// 串流配置（§3.1 `StreamConfig`、§3.11 `set_settings`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamConfig {
    /// 画面宽。
    pub width: u16,
    /// 画面高。
    pub height: u16,
    /// 目标帧率。
    pub fps: u32,
    /// 目标码率，bit/s。
    pub bitrate_bps: u32,
    /// 编解码器。
    pub codec: CodecKind,
    /// 是否开启音频。
    pub audio_enabled: bool,
    /// 特性位图。
    pub features: u64,
}

impl Default for StreamConfig {
    /// 1080p60 / 20 Mbps / H.264（§8 T0 档位）。
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_bps: 20_000_000,
            codec: CodecKind::H264,
            audio_enabled: false,
            features: 0,
        }
    }
}

/// §5 版本协商：主版本不同直接拒绝（`E1001`），次版本取较小值（向后兼容）。
pub fn negotiate_version(
    local: &ProtocolVersion,
    remote: &ProtocolVersion,
) -> Result<ProtocolVersion, ProtoError> {
    if !local.same_major(remote) {
        return Err(ProtoError::VersionIncompatible {
            local: local.major,
            remote: remote.major,
        });
    }
    Ok(ProtocolVersion {
        major: local.major,
        minor: local.minor.min(remote.minor),
    })
}

/// §3.1 能力协商：取双方公共能力，交集为空时返回 `E1004`。
pub fn negotiate(local: &Caps, remote: &Caps) -> Result<Agreed, ProtoError> {
    let codec = local
        .codecs
        .iter()
        .copied()
        .filter(|c| remote.codecs.contains(c))
        .max()
        .ok_or_else(|| ProtoError::CapabilityMismatch {
            reason: "双方无公共编解码器".to_string(),
        })?;

    if local.max_fps == 0 || remote.max_fps == 0 {
        return Err(ProtoError::CapabilityMismatch {
            reason: "帧率能力为零".to_string(),
        });
    }

    Ok(Agreed {
        codec,
        width: local.max_res.width.min(remote.max_res.width),
        height: local.max_res.height.min(remote.max_res.height),
        fps: local.max_fps.min(remote.max_fps),
        features: local.features & remote.features,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_major_negotiates_minor() {
        let local = ProtocolVersion { major: 1, minor: 3 };
        let remote = ProtocolVersion { major: 1, minor: 1 };
        let agreed = negotiate_version(&local, &remote).expect("同主版本应当成功");
        assert_eq!(agreed.major, 1);
        assert_eq!(agreed.minor, 1);
    }

    #[test]
    fn different_major_is_rejected_with_e1001() {
        let local = ProtocolVersion { major: 1, minor: 0 };
        let remote = ProtocolVersion { major: 2, minor: 0 };
        let err = negotiate_version(&local, &remote).expect_err("主版本不同必须拒绝");
        assert_eq!(err.code(), 1001);
    }

    #[test]
    fn negotiate_picks_common_codec_and_minima() {
        let local = Caps {
            codecs: vec![CodecKind::H264, CodecKind::Hevc],
            max_res: Resolution::new(3840, 2160),
            max_fps: 60,
            features: feature::AUDIO | feature::GAMEPAD,
        };
        let remote = Caps {
            codecs: vec![CodecKind::Hevc],
            max_res: Resolution::new(2560, 1440),
            max_fps: 120,
            features: feature::AUDIO,
        };
        let agreed = negotiate(&local, &remote).expect("存在公共编解码器");
        assert_eq!(agreed.codec, CodecKind::Hevc);
        assert_eq!(agreed.width, 2560);
        assert_eq!(agreed.height, 1440);
        assert_eq!(agreed.fps, 60);
        assert_eq!(agreed.features, feature::AUDIO);
    }

    #[test]
    fn negotiate_without_common_codec_is_capability_mismatch() {
        let local = Caps {
            codecs: vec![CodecKind::H264],
            ..Caps::default()
        };
        let remote = Caps {
            codecs: vec![CodecKind::Hevc],
            ..Caps::default()
        };
        let err = negotiate(&local, &remote).expect_err("无公共编解码器");
        assert_eq!(err.code(), 1004);
    }
}
