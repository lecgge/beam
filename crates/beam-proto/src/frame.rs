//! 16 字节大端视频帧头（§5 帧头位宽表）。
//!
//! 位宽严格照 §5：
//!
//! | 字段 | 位宽 | 说明 |
//! |---|---|---|
//! | `ver` | 4 | 当前 1 |
//! | `type` | 4 | 0 视频 1 音频 2 FEC 冗余 |
//! | `flags` | 8 | bit0 关键帧, bit1 帧末片, bit2 切片边界 |
//! | `frame_id` | 32 | 递增 |
//! | `shard_idx` | 16 | 组内分片号 |
//! | `data_shards` | 8 | k |
//! | `fec_shards` | 8 | m |
//! | `capture_ts` | 48 | 主机微秒时钟 |
//!
//! 合计 128 位 = 16 字节，负载上限 1184 B（16 + 1184 = §3.2 的 `max_datagram` 1200）。

use crate::error::ProtoError;

/// 帧头长度（字节）。
pub const FRAME_HEADER_LEN: usize = 16;

/// 单个数据报内负载上限（字节，§5）。
pub const MAX_PAYLOAD_LEN: usize = 1184;

/// `capture_ts` 的位宽掩码（48 位）。
pub const CAPTURE_TS_MASK: u64 = (1u64 << 48) - 1;

/// 帧类型（§5 `type` 4 位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameKind {
    /// 0：视频数据片。
    Video,
    /// 1：音频数据片。
    Audio,
    /// 2：FEC 冗余片。
    Fec,
    /// 3–15：保留，保留位原样透传以便将来扩展。
    Reserved(u8),
}

impl FrameKind {
    /// 线上 4 位取值。
    pub const fn bits(self) -> u8 {
        match self {
            FrameKind::Video => 0,
            FrameKind::Audio => 1,
            FrameKind::Fec => 2,
            FrameKind::Reserved(v) => v & 0x0F,
        }
    }

    /// 由线上 4 位取值还原。
    pub const fn from_bits(v: u8) -> Self {
        match v & 0x0F {
            0 => FrameKind::Video,
            1 => FrameKind::Audio,
            2 => FrameKind::Fec,
            other => FrameKind::Reserved(other),
        }
    }
}

/// 帧标志（§5 `flags` 8 位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FrameFlags(u8);

impl FrameFlags {
    /// bit0：关键帧（IDR）。
    pub const KEYFRAME: u8 = 1 << 0;
    /// bit1：帧末片。
    pub const LAST_SHARD: u8 = 1 << 1;
    /// bit2：切片边界。
    pub const SLICE_BOUNDARY: u8 = 1 << 2;

    /// 空标志。
    pub const fn empty() -> Self {
        Self(0)
    }

    /// 原始位值。
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// 是否包含指定位。
    pub const fn contains(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    /// 置位。
    pub const fn with(self, bit: u8) -> Self {
        Self(self.0 | bit)
    }

    /// 清位。
    pub const fn without(self, bit: u8) -> Self {
        Self(self.0 & !bit)
    }

    /// 是否关键帧（bit0）。
    pub const fn is_keyframe(self) -> bool {
        self.contains(Self::KEYFRAME)
    }

    /// 是否帧末片（bit1）。
    pub const fn is_last_shard(self) -> bool {
        self.contains(Self::LAST_SHARD)
    }

    /// 是否切片边界（bit2）。
    pub const fn is_slice_boundary(self) -> bool {
        self.contains(Self::SLICE_BOUNDARY)
    }
}

/// 16 字节大端视频帧头（§5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoPktHeader {
    /// 协议版本（4 位，当前 1）。
    pub ver: u8,
    /// 帧类型（4 位）。
    pub kind: FrameKind,
    /// 标志位（8 位）。
    pub flags: FrameFlags,
    /// 帧号（32 位，递增）。
    pub frame_id: u32,
    /// 组内分片号（16 位）。
    pub shard_idx: u16,
    /// 数据分片数 k（8 位）。
    pub data_shards: u8,
    /// FEC 冗余分片数 m（8 位）。
    pub fec_shards: u8,
    /// 主机抓屏时刻，微秒（48 位）。
    pub capture_ts: u64,
}

impl VideoPktHeader {
    /// 当前帧头版本号（§5 `ver` 当前 1）。
    pub const VER: u8 = 1;

    /// 构造一个视频帧头（`type=0`，`ver=1`）。
    pub const fn video(frame_id: u32, capture_ts: u64) -> Self {
        Self {
            ver: Self::VER,
            kind: FrameKind::Video,
            flags: FrameFlags::empty(),
            frame_id,
            shard_idx: 0,
            data_shards: 1,
            fec_shards: 0,
            capture_ts: capture_ts & CAPTURE_TS_MASK,
        }
    }

    /// 序列化为 16 字节大端。
    pub fn encode(&self) -> [u8; FRAME_HEADER_LEN] {
        let mut out = [0u8; FRAME_HEADER_LEN];
        out[0] = ((self.ver & 0x0F) << 4) | (self.kind.bits() & 0x0F);
        out[1] = self.flags.bits();
        out[2..6].copy_from_slice(&self.frame_id.to_be_bytes());
        out[6..8].copy_from_slice(&self.shard_idx.to_be_bytes());
        out[8] = self.data_shards;
        out[9] = self.fec_shards;
        let ts = (self.capture_ts & CAPTURE_TS_MASK).to_be_bytes();
        out[10..16].copy_from_slice(&ts[2..8]);
        out
    }

    /// 从字节流解析帧头（长度不足返回 `E1002`）。
    pub fn decode(buf: &[u8]) -> Result<Self, ProtoError> {
        if buf.len() < FRAME_HEADER_LEN {
            return Err(ProtoError::DecodeFailed {
                reason: format!("帧头不足 {FRAME_HEADER_LEN} 字节,实际 {}", buf.len()),
            });
        }
        let mut ts = [0u8; 8];
        ts[2..8].copy_from_slice(&buf[10..16]);
        Ok(Self {
            ver: buf[0] >> 4,
            kind: FrameKind::from_bits(buf[0] & 0x0F),
            flags: FrameFlags(buf[1]),
            frame_id: u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]),
            shard_idx: u16::from_be_bytes([buf[6], buf[7]]),
            data_shards: buf[8],
            fec_shards: buf[9],
            capture_ts: u64::from_be_bytes(ts) & CAPTURE_TS_MASK,
        })
    }

    /// 帧头 + 负载拼成一个数据报。
    pub fn to_datagram(&self, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
        out.extend_from_slice(&self.encode());
        out.extend_from_slice(payload);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_sixteen_bytes() {
        let h = VideoPktHeader::video(1, 0);
        assert_eq!(h.encode().len(), FRAME_HEADER_LEN);
        assert_eq!(FRAME_HEADER_LEN * 8, 128);
    }

    #[test]
    fn bit_layout_is_big_endian_and_exact() {
        let h = VideoPktHeader {
            ver: 0x3,
            kind: FrameKind::Fec,
            flags: FrameFlags::empty()
                .with(FrameFlags::KEYFRAME)
                .with(FrameFlags::LAST_SHARD)
                .with(FrameFlags::SLICE_BOUNDARY),
            frame_id: 0x0102_0304,
            shard_idx: 0x0506,
            data_shards: 0x07,
            fec_shards: 0x08,
            capture_ts: 0x090A_0B0C_0D0E,
        };
        let b = h.encode();
        assert_eq!(b[0], 0x32, "ver=3(type=2) 应为 0b0011_0010");
        assert_eq!(b[1], 0b0000_0111, "flags bit0/bit1/bit2");
        assert_eq!(&b[2..6], &[0x01, 0x02, 0x03, 0x04], "frame_id 大端");
        assert_eq!(&b[6..8], &[0x05, 0x06], "shard_idx 大端");
        assert_eq!(b[8], 0x07, "data_shards");
        assert_eq!(b[9], 0x08, "fec_shards");
        assert_eq!(
            &b[10..16],
            &[0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E],
            "capture_ts 48 位大端"
        );

        let back = VideoPktHeader::decode(&b).expect("应当能解回");
        assert_eq!(back, h);
    }

    #[test]
    fn capture_ts_is_truncated_to_48_bits() {
        let h = VideoPktHeader {
            capture_ts: u64::MAX,
            ..VideoPktHeader::video(0, 0)
        };
        let back = VideoPktHeader::decode(&h.encode()).expect("应当能解回");
        assert_eq!(back.capture_ts, CAPTURE_TS_MASK);
    }

    #[test]
    fn short_buffer_is_decode_failure() {
        let err = VideoPktHeader::decode(&[0u8; 8]).expect_err("长度不足必须失败");
        assert_eq!(err.code(), 1002);
    }

    #[test]
    fn flags_helpers_match_bit_table() {
        let f = FrameFlags::empty()
            .with(FrameFlags::KEYFRAME)
            .with(FrameFlags::SLICE_BOUNDARY);
        assert!(f.is_keyframe());
        assert!(!f.is_last_shard());
        assert!(f.is_slice_boundary());
        assert_eq!(f.bits(), 0b0000_0101);
    }
}
