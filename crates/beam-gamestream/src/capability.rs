//! GameStream 能力映射（§5 兼容策略）。
//!
//! 只描述「GameStream 侧的能力」与「本仓库协议能力」之间的映射，
//! 不含任何来自 GPL 实现的内容。

use beam_proto::caps::{feature, Caps, CodecKind, Resolution};

/// GameStream 侧的服务名（公开可观察的协议行为）。
pub const GAMESTREAM_SERVICE_TYPE: &str = "_nvstream._tcp.local";

/// 本轮实现的 GameStream 协议版本号（v1 预留）。
pub const GAMESTREAM_VERSION: &str = "7.1.431.0";

/// GameStream 主机能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameStreamCaps {
    /// 支持的编解码器（HEVC / H.264）。
    pub codecs: Vec<CodecKind>,
    /// 最大分辨率。
    pub max_res: Resolution,
    /// 最大帧率。
    pub max_fps: u32,
    /// 是否支持音频。
    pub audio: bool,
    /// 是否支持手柄。
    pub gamepad: bool,
}

impl Default for GameStreamCaps {
    fn default() -> Self {
        Self {
            codecs: vec![CodecKind::H264, CodecKind::Hevc],
            max_res: Resolution::new(1920, 1080),
            max_fps: 60,
            audio: true,
            gamepad: true,
        }
    }
}

impl GameStreamCaps {
    /// 映射成本仓库协议的能力集。
    pub fn to_caps(&self) -> Caps {
        let mut features = 0u64;
        if self.audio {
            features |= feature::AUDIO;
        }
        if self.gamepad {
            features |= feature::GAMEPAD;
        }
        Caps {
            codecs: self.codecs.clone(),
            max_res: self.max_res,
            max_fps: self.max_fps,
            features,
        }
    }
}

/// 兼容层入口。
#[derive(Debug, Default)]
pub struct GameStreamCompat {
    caps: GameStreamCaps,
    enabled: bool,
}

impl GameStreamCompat {
    /// 创建兼容层。
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否启用（对应 `gamestream` feature，§7）。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 启用兼容层。
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// 能力集。
    pub fn caps(&self) -> &GameStreamCaps {
        &self.caps
    }

    /// 与本仓库协议端做能力协商。
    ///
    /// 交集为空时返回 `E15002`。
    pub fn negotiate(&self, local: &Caps) -> Result<Caps, u16> {
        if !self.enabled {
            return Err(crate::code::CAPABILITY_INCOMPATIBLE);
        }
        let codecs: Vec<CodecKind> = local
            .codecs
            .iter()
            .copied()
            .filter(|c| self.caps.codecs.contains(c))
            .collect();
        if codecs.is_empty() {
            return Err(crate::code::CAPABILITY_INCOMPATIBLE);
        }
        Ok(Caps {
            codecs,
            max_res: Resolution::new(
                local.max_res.width.min(self.caps.max_res.width),
                local.max_res.height.min(self.caps.max_res.height),
            ),
            max_fps: local.max_fps.min(self.caps.max_fps),
            features: local.features & self.caps.to_caps().features,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_map_features() {
        let caps = GameStreamCaps::default().to_caps();
        assert!(caps.has_feature(feature::AUDIO));
        assert!(caps.has_feature(feature::GAMEPAD));
        assert!(!caps.has_feature(feature::HDR));
    }

    #[test]
    fn negotiate_requires_enable() {
        let compat = GameStreamCompat::new();
        let err = compat
            .negotiate(&Caps::default())
            .expect_err("未启用必须失败");
        assert_eq!(err, crate::code::CAPABILITY_INCOMPATIBLE);
    }

    #[test]
    fn negotiate_picks_common_codec() {
        let mut compat = GameStreamCompat::new();
        compat.enable();
        let local = Caps {
            codecs: vec![CodecKind::Hevc],
            max_res: Resolution::new(3840, 2160),
            max_fps: 120,
            features: feature::AUDIO,
        };
        let agreed = compat.negotiate(&local).expect("应当能协商");
        assert_eq!(agreed.codecs, vec![CodecKind::Hevc]);
        assert_eq!(agreed.max_res, Resolution::new(1920, 1080));
        assert_eq!(agreed.max_fps, 60);
    }

    #[test]
    fn negotiate_without_common_codec_fails() {
        let mut compat = GameStreamCompat::new();
        compat.enable();
        let local = Caps {
            codecs: vec![CodecKind::Av1],
            ..Caps::default()
        };
        let err = compat.negotiate(&local).expect_err("无交集必须失败");
        assert_eq!(err, crate::code::CAPABILITY_INCOMPATIBLE);
    }
}
