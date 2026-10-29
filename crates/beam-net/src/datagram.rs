//! datagram 分包与重组（§3.2、§5）。
//!
//! ## 分包
//!
//! - 每个数据报 = 16 字节帧头（§5）+ 负载，负载 ≤ 1184 B（`max_datagram=1200`）；
//! - 单个 FEC 组的数据片 `k ≤ 200`（§5），`m = ceil(k × ratio)`；
//! - 一帧超过 200 × 1184 B 时拆成多个 FEC 组（§5），各组共享同一 `k`/`m`；
//! - `shard_idx` 是**组内分片号**（0..k+m-1，§5 帧头表）；
//! - 冗余片用帧头 `type = 2`（FEC 冗余）标记。
//!
//! ## 组号是怎么恢复的
//!
//! §5 帧头表规定 `shard_idx` 是「组内分片号」，而 16 字节帧头的位宽已经占满，
//! 没有空闲字段可以直接放「组号」。本实现的方案是：
//!
//! **组号放在帧头 `flags` 字节的 bit3..bit7。**
//!
//! - §5 帧头表只命名了 `flags` 的 bit0（关键帧）、bit1（帧末片）、bit2（切片边界），
//!   bit3..bit7 原本闲置；这里给它们赋予「组号」语义；
//! - `flags` 位宽仍是 8 位、帧头仍是 16 字节，**帧头布局与各字段位宽都没有变化**，
//!   变的只是 `flags` 里那 5 个原本没有命名的位；
//! - 发送端 [`packetize`] 给第 `g` 组的**每一片**都写入组号 `g`；
//! - 接收端用 [`group_index`] 从 `flags` 读出组号，`shard_idx` 直接就是组内号。
//!
//! 因为组号是「每片自带」的，接收端完全不依赖到达顺序：
//! **丢包、乱序、重复都不会让组边界错乱**。
//!
//! 组号 5 位 ⇒ 单帧最多 [`MAX_GROUPS_PER_FRAME`] = 32 组，
//! 单帧上限 = 32 × 200 × 1184 B ≈ 7.2 MiB；
//! 超过时 [`packetize`] 直接报错，不会发出接收端无法还原组号的帧。
//!
//! ## 拆组上限：200 而不是 255（§5 的自相矛盾之处，按代码现状记录）
//!
//! §5 同时写了「`k ≤ 200`」与「单帧超过 255 片时拆成多个组」两条**互斥**规则。
//! 本 crate 的现状是按 **200** 拆组：
//!
//! - 拆组容量 = `MAX_DATA_SHARDS(200) × max_payload`（[`crate::fec::MAX_DATA_SHARDS`]）；
//! - 255 只用来限制 `k + m`（单组总分片数，因为帧头 `data_shards`/`fec_shards` 各 8 位），
//!   见 [`crate::fec::MAX_SHARDS_PER_GROUP`]。
//!
//! 换句话说：**「255 片」是单组总分片上限，不是拆组阈值**。
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
//! 缺片时用 FEC 冗余恢复（仅 `fec` feature 打开时）；
//! 超过 `frame_timeout_us` 的帧按 `E2004` 过期丢弃。

use std::collections::HashMap;

use bytes::Bytes;

use beam_proto::frame::{FrameFlags, FrameKind, VideoPktHeader, FRAME_HEADER_LEN};
use beam_proto::AssembledFrame;

use crate::error::NetError;
use crate::fec::{effective_parity_shard_count, FecCodec, FecGroup};

/// 负载长度前缀字节数。
const LEN_PREFIX: usize = 4;

/// §5 帧头 `flags` 字节里「组号」所在的位偏移。
///
/// §5 帧头表只命名了 bit0（关键帧）、bit1（帧末片）、bit2（切片边界），
/// bit3..bit7 闲置；这里承载帧内 FEC 组号（见模块文档「组号是怎么恢复的」）。
pub const GROUP_INDEX_SHIFT: u32 = 3;

/// 组号掩码（`flags` 的 bit3..bit7）。
pub const GROUP_INDEX_MASK: u8 = 0b1111_1000;

/// 单帧允许的最大 FEC 组数（组号 5 位 ⇒ 0..=31）。
pub const MAX_GROUPS_PER_FRAME: usize = 32;

fn ceil_div(a: usize, b: usize) -> usize {
    debug_assert!(b > 0);
    a.div_ceil(b)
}

/// 把组号写进帧头 `flags`（bit3..bit7）。
///
/// 组号超出 5 位时按位截断（调用方应先用 [`MAX_GROUPS_PER_FRAME`] 校验）。
pub fn with_group_index(flags: FrameFlags, group: usize) -> FrameFlags {
    let g = (group as u8) & (GROUP_INDEX_MASK >> GROUP_INDEX_SHIFT);
    flags.with(g << GROUP_INDEX_SHIFT)
}

/// 从帧头 `flags` 读出组号（bit3..bit7）。
pub fn group_index(flags: FrameFlags) -> usize {
    ((flags.bits() & GROUP_INDEX_MASK) >> GROUP_INDEX_SHIFT) as usize
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
/// `slice_boundary` 对应帧头 flags bit2（切片边界，§5）：置位时**只在整帧的第一片**
/// （第 0 组第 0 片）打标，表示「本帧是一个新切片的起点」。
///
/// `ratio` 是 FEC 冗余比例；`fec` feature 关闭（MVP）时不生效，
/// 发送端不会产生任何冗余分片，详见 [`crate::fec::effective_parity_shard_count`]。
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
    let group_capacity = crate::fec::MAX_DATA_SHARDS * max_payload;
    let groups = ceil_div(total, group_capacity).max(1);
    // 组号只有 5 位（flags bit3..bit7），超过上限的帧发不出去。
    if groups > MAX_GROUPS_PER_FRAME {
        return Err(NetError::ReassemblyFailed {
            reason: format!(
                "单帧需要 {groups} 个 FEC 组,超过上限 {MAX_GROUPS_PER_FRAME}(帧头组号只有 5 位)"
            ),
        });
    }
    // 每组片数 k（≤ MAX_DATA_SHARDS）。
    let k = ceil_div(ceil_div(total, groups), max_payload).max(1);
    // 每片长度（等长，RS 要求）。
    let shard_len = ceil_div(total, groups * k).max(1);
    // 冗余片数：feature 关闭时恒为 0（MVP 不产生冗余分片）。
    let m = effective_parity_shard_count(k, ratio);

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
            let mut flags = FrameFlags::empty();
            if frame.is_key {
                flags = flags.with(FrameFlags::KEYFRAME);
            }
            if slice_boundary && g == 0 && local == 0 {
                flags = flags.with(FrameFlags::SLICE_BOUNDARY);
            }
            let is_last = g == groups - 1 && local == group_len - 1;
            if is_last {
                flags = flags.with(FrameFlags::LAST_SHARD);
            }
            // 组号写进 flags bit3..bit7，接收端据此还原 FEC 组边界。
            flags = with_group_index(flags, g);
            let header = VideoPktHeader {
                ver: VideoPktHeader::VER,
                kind: if is_parity {
                    FrameKind::Fec
                } else {
                    FrameKind::Video
                },
                flags,
                frame_id: frame.frame_id,
                // §5 帧头表：shard_idx 是「组内分片号」。
                shard_idx: local as u16,
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
    is_key: bool,
    /// 第一片到达的本地单调时刻（微秒），超时判定从这一刻起算。
    arrival_us: u64,
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
    ///
    /// 组号来自帧头 `flags` 的 bit3..bit7，组内号来自 `shard_idx`（§5 帧头表）。
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
        // 组号：flags bit3..bit7（每片自带，不依赖到达顺序）。
        let group_no = group_index(header.flags);
        // 组内号：shard_idx（§5 帧头表「组内分片号」）。
        let local = header.shard_idx as usize;
        if local >= group_len {
            return Err(NetError::ReassemblyFailed {
                reason: format!("组内分片号 {local} 超出组长 {group_len}"),
            });
        }

        let frame_id = header.frame_id;
        if !self.frames.contains_key(&frame_id) && self.frames.len() >= self.max_frames {
            self.drop_oldest_frame();
        }
        let entry = self
            .frames
            .entry(frame_id)
            .or_insert_with(|| FrameAssembly {
                k,
                m,
                is_key: header.flags.is_keyframe(),
                arrival_us: crate::now_us(),
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
            .entry(group_no)
            .or_insert_with(|| FecGroup::new(k, m, payload.len().max(1)));
        // 重复片：直接忽略（§3.2 测试要求「重复检测」）。
        group.insert(local, payload);

        if header.flags.is_last_shard() {
            entry.last_group = Some(group_no);
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
                // FEC 恢复只在 `fec` feature 打开时可用；关闭时缺片就是缺片。
                if crate::fec::FEC_ENABLED && group.is_recoverable() {
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

    /// 清理超时帧，返回被丢弃的帧号（调用方应记为 `E2004`）。
    ///
    /// `now_us` 是**本地单调时钟**读数（见 [`crate::now_us`]），
    /// 帧的超时从「第一片到达」开始计时；这样与主机的 `capture_ts` 时钟无关，
    /// 不会因为两端时钟不同步而误判。
    pub fn expire(&mut self, now_us: u64) -> Vec<u32> {
        let timeout = self.frame_timeout_us;
        let expired: Vec<u32> = self
            .frames
            .iter()
            .filter(|(_, f)| now_us.saturating_sub(f.arrival_us) > timeout)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            self.frames.remove(id);
        }
        expired
    }

    /// 按本地单调时钟清理超时帧，返回被丢弃的帧号（调用方应记为 `E2004`）。
    pub fn expire_stale(&mut self) -> Vec<u32> {
        self.expire(crate::now_us())
    }

    /// 当前正在重组的帧数。
    pub fn pending_frames(&self) -> usize {
        self.frames.len()
    }

    /// 帧数超过 `max_frames` 时丢弃最早到达的一帧（限制内存占用）。
    fn drop_oldest_frame(&mut self) {
        let oldest = self
            .frames
            .iter()
            .min_by_key(|(_, f)| f.arrival_us)
            .map(|(id, _)| *id);
        if let Some(id) = oldest {
            self.frames.remove(&id);
        }
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

    /// 需要多个 FEC 组才能装下的负载（> 200 × 1184 B）。
    fn multi_group_payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
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
    #[cfg(feature = "fec")]
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
        // 2000 字节 ⇒ 至少两片，投一片后帧保持未完成（FEC 开关两种情况都成立）。
        let f = frame(vec![1u8; 2000]);
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");
        assert!(packets.len() >= 2);
        // 只投第一片，帧保持未完成。
        let _ = r
            .push(&packets[0].header, &packets[0].payload)
            .expect("不应报错");
        assert_eq!(r.pending_frames(), 1);
        // 刚到的帧还不该超时。
        assert!(r.expire(crate::now_us()).is_empty());
        // 超时口径是本地单调时钟，传一个足够大的读数必然超时。
        let expired = r.expire(u64::MAX / 2);
        assert_eq!(expired, vec![42]);
        assert_eq!(r.pending_frames(), 0);
    }

    // ---- 组号恢复（§5「组内分片号」）相关用例 ----

    #[test]
    fn shard_idx_is_group_local_and_group_index_travels_in_flags() {
        let payload = multi_group_payload(700_000);
        let f = frame(payload);
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        // 至少 3 个组。
        let groups: std::collections::BTreeSet<usize> = packets
            .iter()
            .map(|p| group_index(p.header.flags))
            .collect();
        assert!(
            groups.len() >= 3,
            "应当拆成多个 FEC 组,实际 {}",
            groups.len()
        );
        assert_eq!(groups.iter().copied().max().unwrap(), groups.len() - 1);

        // shard_idx 只在组内递增，且始终小于组长。
        let mut per_group: std::collections::BTreeMap<usize, Vec<u16>> = Default::default();
        for p in &packets {
            let g = group_index(p.header.flags);
            let group_len = p.header.data_shards as u16 + p.header.fec_shards as u16;
            assert!(
                p.header.shard_idx < group_len,
                "shard_idx 必须是组内分片号(组 {g} 组长 {group_len})"
            );
            per_group.entry(g).or_default().push(p.header.shard_idx);
        }
        for (g, idx) in &per_group {
            let mut sorted = idx.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                sorted,
                (0..sorted.len() as u16).collect::<Vec<_>>(),
                "第 {g} 组的组内分片号必须是从 0 开始的连续序列"
            );
        }
    }

    #[test]
    fn multi_group_frame_reassembles_in_order() {
        let payload = multi_group_payload(700_000);
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        let mut r = Reassembler::new();
        let mut out = None;
        for p in &packets {
            if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                out = Some(frame);
            }
        }
        assert_eq!(
            out.expect("多组帧应当重组出来").payload.as_ref(),
            payload.as_slice()
        );
    }

    #[test]
    fn multi_group_frame_reassembles_when_shuffled_and_duplicated() {
        let payload = multi_group_payload(700_000);
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        // 乱序：按组号交叉打乱（组 1 的片插到组 0 的片中间）。
        let mut shuffled: Vec<Packet> = Vec::with_capacity(packets.len());
        let mut by_group: std::collections::BTreeMap<usize, Vec<Packet>> = Default::default();
        for p in &packets {
            by_group
                .entry(group_index(p.header.flags))
                .or_default()
                .push(p.clone());
        }
        let group_ids: Vec<usize> = by_group.keys().copied().collect();
        let mut cursors = vec![0usize; group_ids.len()];
        loop {
            let mut progressed = false;
            for (i, g) in group_ids.iter().enumerate() {
                let list = &by_group[g];
                if cursors[i] < list.len() {
                    shuffled.push(list[cursors[i]].clone());
                    cursors[i] += 1;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        let mut r = Reassembler::new();
        let mut out = None;
        for (i, p) in shuffled.iter().enumerate() {
            // 一半的分片重复投一次。
            let times = if i % 2 == 0 { 2 } else { 1 };
            for _ in 0..times {
                if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                    out = Some(frame);
                }
            }
        }
        assert_eq!(
            out.expect("乱序 + 重复下多组帧仍应重组正确")
                .payload
                .as_ref(),
            payload.as_slice()
        );
    }

    #[test]
    #[cfg(feature = "fec")]
    fn multi_group_frame_recovers_lost_shards_in_every_group() {
        let payload = multi_group_payload(700_000);
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        // 每个组各丢 1 片数据片，其余靠 FEC 恢复。
        let mut dropped_per_group: std::collections::BTreeMap<usize, usize> = Default::default();
        let mut r = Reassembler::new();
        let mut out = None;
        for p in &packets {
            let g = group_index(p.header.flags);
            let is_data = matches!(p.header.kind, FrameKind::Video);
            let dropped = dropped_per_group.entry(g).or_insert(0);
            if is_data && *dropped == 0 {
                *dropped += 1;
                continue;
            }
            if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                out = Some(frame);
            }
        }
        assert!(
            dropped_per_group.values().all(|n| *n == 1),
            "每个组都应当丢掉过 1 片"
        );
        assert!(dropped_per_group.len() >= 3, "用例应当覆盖 3 个以上的组");
        assert_eq!(
            out.expect("FEC 应当恢复出多组完整帧").payload.as_ref(),
            payload.as_slice()
        );
    }

    #[test]
    #[cfg(not(feature = "fec"))]
    fn multi_group_frame_with_lost_shard_is_never_mis_assembled() {
        // MVP（无 FEC）：丢掉一片就没有冗余可补，整帧必须保持「未完成」，
        // 绝不能把别的组的数据拼进来凑出一个错误的帧。
        let payload = multi_group_payload(700_000);
        let f = frame(payload.clone());
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        let mut r = Reassembler::new();
        let mut out = None;
        let mut dropped = 0usize;
        for p in &packets {
            if dropped == 0 && matches!(p.header.kind, FrameKind::Video) {
                dropped += 1;
                continue;
            }
            if let Some(frame) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                out = Some(frame);
            }
        }
        assert_eq!(dropped, 1);
        assert!(
            out.is_none(),
            "缺片且无 FEC 时不得拼出错误的帧,实际拼出了 frame_id = {:?}",
            out.map(|f| f.frame_id)
        );
        // 该帧仍在重组器里等待补齐。
        assert_eq!(r.pending_frames(), 1);
    }

    #[test]
    fn shard_idx_out_of_range_is_rejected() {
        let f = frame(vec![1u8; 100]);
        let packets = packetize(&f, 0.2, 1200, false).expect("应当能分包");
        let mut r = Reassembler::new();
        // 伪造一个组内号越界的分片。
        let mut header = packets[0].header;
        header.shard_idx = 250;
        let err = r
            .push(&header, &packets[0].payload)
            .expect_err("组内分片号越界必须报错");
        assert_eq!(err.code(), 2005);
    }

    #[test]
    fn frames_beyond_group_limit_are_rejected() {
        // 32 组上限：33 组需要 > 32 × 200 × 1184 B。
        let payload = multi_group_payload(8_000_000);
        let f = frame(payload);
        let err = packetize(&f, 0.2, 1200, false).expect_err("超过组数上限必须报错");
        assert_eq!(err.code(), 2005);
    }
}
