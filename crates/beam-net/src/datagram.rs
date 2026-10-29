//! datagram 分包与重组（§3.2、§5）。
//!
//! ## 分包
//!
//! - 每个数据报 = 16 字节帧头（§5）+ 负载，负载 ≤ 1184 B（`max_datagram=1200`）；
//! - 单个 FEC 组的数据片 `k ≤ 200`（§5），`m = ceil(k × ratio)`；
//! - 一帧超过 200 × 1184 B 时拆成多个 FEC 组（§5「单帧超过 255 片时拆成多个组」），
//!   各组共享同一 `k`/`m`，`shard_idx` 是帧内全局分片号，
//!   组号 = `shard_idx / (k + m)`，组内号 = `shard_idx % (k + m)`；
//! - 冗余片用帧头 `type = 2`（FEC 冗余）标记。
//!
//! ## 负载长度前缀（实现约定）
//!
//! RS 要求同组各片等长，因此重组后需要知道原始负载长度。
//! 发送端在整帧负载最前面加 4 字节大端长度，接收端据此裁掉填充。
//! 数据片内容不会被 RS 改写，所以长度前缀可以被完整还原。
//!
//! ## 重组
//!
//! 处理乱序、丢包、重复；重复片直接丢弃；
//! 缺片时用 FEC 冗余恢复；超过 `frame_timeout_us` 的帧按 `E2004` 过期丢弃。

use std::collections::HashMap;

use bytes::Bytes;

use beam_proto::frame::{FrameFlags, FrameKind, VideoPktHeader, FRAME_HEADER_LEN};
use beam_proto::AssembledFrame;

use crate::error::NetError;
use crate::fec::{parity_shard_count, FecCodec, FecGroup, MAX_DATA_SHARDS};

/// 负载长度前缀字节数。
const LEN_PREFIX: usize = 4;

fn ceil_div(a: usize, b: usize) -> usize {
    debug_assert!(b > 0);
    a.div_ceil(b)
}

/// 一个待发送的数据报。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// 帧头。
    pub header: VideoPktHeader,
    /// 负载（不含帧头）。
    pub payload: Vec<u8>,
}

impl Packet {
    /// 拼成完整的线上数据报。
    pub fn to_datagram(&self) -> Vec<u8> {
        self.header.to_datagram(&self.payload)
    }
}

/// 把一帧编码后的画面切成一组数据报。
///
/// `slice_boundary` 对应帧头 flags bit2（切片边界，§5）。
pub fn packetize(
    frame: &beam_proto::EncodedFrame,
    ratio: f32,
    max_datagram: usize,
    slice_boundary: bool,
) -> Result<Vec<Packet>, NetError> {
    let max_payload = max_datagram.saturating_sub(FRAME_HEADER_LEN).max(1);
    let payload = frame.to_payload();
    let payload: &[u8] = &payload;
    let total = payload.len() + LEN_PREFIX;
    if total == LEN_PREFIX && payload.is_empty() {
        return Err(NetError::ReassemblyFailed {
            reason: "空帧不可分包".to_string(),
        });
    }

    // 组数：每组最多 MAX_DATA_SHARDS 片，每片最多 max_payload 字节。
    let group_capacity = MAX_DATA_SHARDS * max_payload;
    let groups = ceil_div(total, group_capacity).max(1);
    // 每组片数 k（≤ MAX_DATA_SHARDS）。
    let k = ceil_div(ceil_div(total, groups), max_payload).max(1);
    // 每片长度（等长，RS 要求）。
    let shard_len = ceil_div(total, groups * k).max(1);
    let m = parity_shard_count(k, ratio);

    // 构造带长度前缀的填充负载。
    let padded_len = groups * k * shard_len;
    let mut padded = vec![0u8; padded_len];
    padded[..LEN_PREFIX].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    padded[LEN_PREFIX..LEN_PREFIX + payload.len()].copy_from_slice(payload);

    let mut codec = FecCodec::new();
    let group_len = k + m;
    let mut packets = Vec::with_capacity(groups * group_len);

    for g in 0..groups {
        let base = g * k * shard_len;
        let data: Vec<Vec<u8>> = (0..k)
            .map(|i| padded[base + i * shard_len..base + (i + 1) * shard_len].to_vec())
            .collect();
        let mut fec = FecGroup::encode(&data, ratio).map_err(|e| NetError::ReassemblyFailed {
            reason: format!("建立 FEC 组失败: {e}"),
        })?;
        codec.encode_group(&mut fec)?;

        for local in 0..group_len {
            let is_parity = local >= k;
            let shard = fec.shards()[local].clone().unwrap_or_default();
            let global = g * group_len + local;
            let mut flags = FrameFlags::empty();
            if frame.is_key {
                flags = flags.with(FrameFlags::KEYFRAME);
            }
            if slice_boundary && local == 0 {
                flags = flags.with(FrameFlags::SLICE_BOUNDARY);
            }
            let is_last = g == groups - 1 && local == group_len - 1;
            if is_last {
                flags = flags.with(FrameFlags::LAST_SHARD);
            }
            let header = VideoPktHeader {
                ver: VideoPktHeader::VER,
                kind: if is_parity {
                    FrameKind::Fec
                } else {
                    FrameKind::Video
                },
                flags,
                frame_id: frame.frame_id,
                shard_idx: global as u16,
                data_shards: k as u8,
                fec_shards: m as u8,
                capture_ts: frame.capture_ts_us,
            };
            packets.push(Packet {
                header,
                payload: shard,
            });
        }
    }

    Ok(packets)
}

/// 分包输入的只读视图（便于在测试与工具里直接构造分包输入）。
#[derive(Debug, Clone, Copy)]
pub struct EncodedFrameView<'a> {
    /// 帧号。
    pub frame_id: u32,
    /// 主机抓屏时刻，微秒。
    pub capture_ts_us: u64,
    /// 是否关键帧。
    pub is_key: bool,
    /// 负载。
    pub payload: &'a [u8],
}

/// 重组中的单帧状态。
#[derive(Debug)]
struct FrameAssembly {
    k: usize,
    m: usize,
    capture_ts: u64,
    is_key: bool,
    groups: HashMap<usize, FecGroup>,
    last_group: Option<usize>,
}

/// 帧重组器（处理乱序 / 丢包 / 重复）。
#[derive(Debug)]
pub struct Reassembler {
    frames: HashMap<u32, FrameAssembly>,
    codec: FecCodec,
    /// 帧超时（微秒）。超时帧按 `E2004` 丢弃。
    pub frame_timeout_us: u64,
    /// 同时保留的最大帧数，超出后丢弃最旧的。
    pub max_frames: usize,
}

impl Default for Reassembler {
    fn default() -> Self {
        Self {
            frames: HashMap::new(),
            codec: FecCodec::new(),
            frame_timeout_us: 500_000,
            max_frames: 64,
        }
    }
}

impl Reassembler {
    /// 新建重组器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 投喂一个数据报；帧完整时返回 `Some`。
    pub fn push_datagram(&mut self, datagram: &[u8]) -> Result<Option<AssembledFrame>, NetError> {
        let header = VideoPktHeader::decode(datagram)?;
        self.push(&header, &datagram[FRAME_HEADER_LEN..])
    }

    /// 投喂一帧头 + 负载；帧完整时返回 `Some`。
    pub fn push(
        &mut self,
        header: &VideoPktHeader,
        payload: &[u8],
    ) -> Result<Option<AssembledFrame>, NetError> {
        let k = header.data_shards as usize;
        let m = header.fec_shards as usize;
        if k == 0 {
            return Err(NetError::ReassemblyFailed {
                reason: "data_shards 为 0".to_string(),
            });
        }
        let group_len = k + m;
        if group_len == 0 {
            return Err(NetError::ReassemblyFailed {
                reason: "分组长度为 0".to_string(),
            });
        }
        let group_index = (header.shard_idx as usize) / group_len;
        let local = (header.shard_idx as usize) % group_len;

        let frame_id = header.frame_id;
        let entry = self
            .frames
            .entry(frame_id)
            .or_insert_with(|| FrameAssembly {
                k,
                m,
                capture_ts: header.capture_ts,
                is_key: header.flags.is_keyframe(),
                groups: HashMap::new(),
                last_group: None,
            });
        if entry.k != k || entry.m != m {
            // 同一帧号出现不同分组参数，说明对端状态错乱，丢弃该帧重新开始。
            self.frames.remove(&frame_id);
            return Ok(None);
        }

        let group = entry
            .groups
            .entry(group_index)
            .or_insert_with(|| FecGroup::new(k, m, payload.len().max(1)));
        // 重复片：直接忽略（§3.2 测试要求「重复检测」）。
        group.insert(local, payload);

        if header.flags.is_last_shard() {
            entry.last_group = Some(group_index);
        }

        let last_group = match entry.last_group {
            Some(g) => g,
            None => return Ok(None),
        };

        // 尝试恢复并判断是否完整。
        let mut complete = true;
        for g in 0..=last_group {
            let group = match entry.groups.get_mut(&g) {
                Some(group) => group,
                None => {
                    complete = false;
                    break;
                }
            };
            if !group.is_complete() {
                if group.is_recoverable() {
                    if self.codec.reconstruct_group(group).is_err() {
                        complete = false;
                        break;
                    }
                } else {
                    complete = false;
                    break;
                }
            }
            if !group.is_complete() {
                complete = false;
                break;
            }
        }
        if !complete {
            return Ok(None);
        }

        // 拼回原始负载。
        let mut padded = Vec::new();
        for g in 0..=last_group {
            let group = entry.groups.get(&g).expect("完整性检查已保证存在");
            for shard in group.data_shards() {
                padded.extend_from_slice(shard);
            }
        }
        if padded.len() < LEN_PREFIX {
            return Err(NetError::ReassemblyFailed {
                reason: "重组结果过短".to_string(),
            });
        }
        let len = u32::from_be_bytes([padded[0], padded[1], padded[2], padded[3]]) as usize;
        let end = (LEN_PREFIX + len).min(padded.len());

        let frame = AssembledFrame {
            frame_id,
            pts_us: header.capture_ts,
            capture_ts_us: header.capture_ts,
            is_key: entry.is_key,
            payload: Bytes::copy_from_slice(&padded[LEN_PREFIX..end]),
        };
        self.frames.remove(&frame_id);
        Ok(Some(frame))
    }

    /// 清理过期帧，返回被丢弃的帧号（调用方应记为 `E2004`）。
    pub fn expire(&mut self, now_us: u64) -> Vec<u32> {
        let timeout = self.frame_timeout_us;
        let expired: Vec<u32> = self
            .frames
            .iter()
            .filter(|(_, f)| now_us.saturating_sub(f.capture_ts) > timeout)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            self.frames.remove(id);
        }
        expired
    }

    /// 当前正在重组的帧数。
    pub fn pending_frames(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_proto::EncodedFrame;

    fn frame(payload: Vec<u8>) -> EncodedFrame {
        EncodedFrame {
            frame_id: 42,
            pts_us: 1000,
            capture_ts_us: 2000,
            is_key: true,
            slices: vec![Bytes::from(payload)],
        }
    }

    #[test]
    fn packetize_respects_max_datagram() {
        let payload = vec![0xABu8; 5000];
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");
        assert!(packets.len() > 1);
        for p in &packets {
            assert!(p.to_datagram().len() <= 1200, "数据报不得超过 max_datagram");
            assert!(p.payload.len() <= 1184);
        }
    }

    #[test]
    fn round_trip_restores_payload() {
        let payload: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        let mut r = Reassembler::new();
        let mut out = None;
        for p in &packets {
            if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                out = Some(frame);
            }
        }
        let out = out.expect("应当重组出完整帧");
        assert_eq!(out.frame_id, 42);
        assert_eq!(out.capture_ts_us, 2000);
        assert!(out.is_key);
        assert_eq!(out.payload.as_ref(), payload.as_slice());
    }

    #[test]
    fn out_of_order_delivery_still_reassembles() {
        let payload: Vec<u8> = (0..4000u32).map(|i| (i % 97) as u8).collect();
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");
        let mut shuffled: Vec<Packet> = packets.clone();
        shuffled.reverse();

        let mut r = Reassembler::new();
        let mut out = None;
        for p in &shuffled {
            if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                out = Some(frame);
            }
        }
        assert_eq!(
            out.expect("应当重组出完整帧").payload.as_ref(),
            payload.as_slice()
        );
    }

    #[test]
    fn duplicate_shards_are_ignored() {
        let payload = vec![7u8; 2000];
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");
        let mut r = Reassembler::new();
        let mut out = None;
        for p in &packets {
            // 每个分片投两次。
            for _ in 0..2 {
                if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                    out = Some(frame);
                }
            }
        }
        assert_eq!(
            out.expect("应当重组出完整帧").payload.as_ref(),
            payload.as_slice()
        );
    }

    #[test]
    fn fec_recovers_lost_shards() {
        let payload: Vec<u8> = (0..6000u32).map(|i| (i % 199) as u8).collect();
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        // 丢掉 2 个数据分片，其余靠 FEC 恢复。
        let mut dropped = 0;
        let mut r = Reassembler::new();
        let mut out = None;
        for p in &packets {
            let is_data = matches!(p.header.kind, FrameKind::Video);
            if is_data && dropped < 2 {
                dropped += 1;
                continue;
            }
            if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                out = Some(frame);
            }
        }
        assert_eq!(dropped, 2);
        assert_eq!(
            out.expect("FEC 应当恢复出完整帧").payload.as_ref(),
            payload.as_slice()
        );
    }

    #[test]
    fn expired_frames_are_reported() {
        let mut r = Reassembler::new();
        let f = frame(vec![1u8; 100]);
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");
        // 只投第一片，帧保持未完成。
        let _ = r
            .push(&packets[0].header, &packets[0].payload)
            .expect("不应报错");
        assert_eq!(r.pending_frames(), 1);
        let expired = r.expire(2_000_000 + r.frame_timeout_us + 1);
        assert_eq!(expired, vec![42]);
        assert_eq!(r.pending_frames(), 0);
    }
}
