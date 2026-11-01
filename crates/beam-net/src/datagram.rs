//! datagram 分包与重组（§3.2、§5）。
//!
//! ## 分包
//!
//! - 每个数据报 = 16 字节帧头（§5）+ 负载，负载 ≤ 1184 B（`max_datagram=1200`）；
//! - 单个 FEC 组的数据片 `k`（§5 的分组粒度，实现取 64），`m = ceil(k × ratio)`；
//! - 单组总分片 `k + m ≤ 255`（§5）；一帧超过单组容量时拆成多个 FEC 组（§5），
//!   各组共享同一 `k`/`m`；
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
//! 单帧上限 = 32 × 64 × 1184 B ≈ 2.4 MiB；
//! 超过时 [`packetize`] 直接报错，不会发出接收端无法还原组号的帧。
//!
//! ## 拆组口径（裁决 S3/S14，已写进 §5）
//!
//! §5 原来同时写了「`k ≤ 200`」与「单帧超过 255 片时拆成多个组」两条**互斥**规则，
//! 现在统一为：
//!
//! - **单组总分片数 `k + m ≤ 255`**（帧头 `data_shards`/`fec_shards` 各 8 位），
//!   见 [`crate::fec::MAX_SHARDS_PER_GROUP`]；
//! - **`k` 是分组粒度**，由实现按 §3.2 的「分包+FEC 编码」性能预算来选，
//!   见 [`crate::fec::MAX_DATA_SHARDS`]。
//!
//! 拆组容量 = `MAX_DATA_SHARDS × max_payload`。
//!
//! **为什么 `k = 64`**：RS 编码代价 ∝ `k`（`m ≈ ratio×k` 个冗余片，
//! 每个是 `k` 个数据片的线性组合，而每片长度固定 ≈ `max_payload`），
//! 即 `代价 ≈ ratio × k × 帧字节数`。
//! 实测 T1 4K60（312 500 B/帧、ratio 0.2）：
//! `k=132` → 1.553 ms（超 §3.2 的 0.8 ms 预算）；
//! `k=64` → **0.670 ms（达标）**。T2 / T3 同样达标。
//! 代价是每帧组数变多（T1 从 2 组到 5 组），组号 5 位上限 32 组仍然够用。
//! **帧头布局、分片编号、组号编码都没有变** —— 变的只是每组放多少片。
//!
//! ## 负载长度前缀（实现约定）
//!
//! RS 要求同组各片等长，因此重组后需要知道原始负载长度。
//! 发送端在整帧负载最前面加一个 8 字节大端前缀，接收端据此裁掉填充：
//!
//! | 偏移 | 长度 | 含义 |
//! |---|---|---|
//! | 0 | 4 | 原始负载字节数 |
//! | 4 | 4 | 本帧的 FEC 组数（总组数） |
//!
//! 数据片内容不会被 RS 改写，所以长度前缀可以被完整还原。
//!
//! ### 为什么「组数」也要放进前缀
//!
//! 帧头里只有一个数据报带 `LAST_SHARD` 标志（bit1），
//! 而**这个数据报丢了，接收端就再也判断不出帧在哪里结束** ——
//! 即使 FEC 的冗余足够、其余分片全都能恢复，整帧也会一直挂在重组器里，
//! 最后按 `E2004` 丢弃。这是实测出来的真实缺陷（见 `tests/fec_recovery.rs`
//! 的恢复率扫描：只丢 1 片时恢复率只有 94% 而不是 100%）。
//!
//! 把「总组数」放进负载前缀可以根治它：
//!
//! - 前缀在**数据片**里，因此它和其它数据片一样**受 FEC 保护**，
//!   丢了也能恢复出来；
//! - 它在**组 0 的第 0 片**里，而组 0 的第 0 片丢了也能靠 FEC 恢复，
//!   于是「帧的跨度」在任何可恢复的丢包下都能拿到；
//! - 帧头的位宽与语义**完全没有变化**（§5 的表一字未改），
//!   `LAST_SHARD` 标志照旧发送、照旧被接收端用作快路径与交叉校验。
//!
//! 换句话说：**组边界不再依赖「某一个数据报有没有到」**，
//! 与 D3 裁决的「组号在帧头 flags、组边界不依赖到达顺序」是同一个方向。
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

/// 负载长度前缀字节数（4 字节负载长度 + 4 字节总组数）。
const LEN_PREFIX: usize = 8;

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
///
/// ## 性能：生产路径请用 [`packetize_with_codec`]
///
/// 本函数**每次调用都新建一个 [`FecCodec`]**，因此它内部的 RS 矩阵缓存
/// 在调用之间**不共享** —— 而 `ReedSolomon::new(k, m)` 要在 GF(2⁸) 上
/// 求逆一个 `k×k` 矩阵（O(k³)），实测 T1 档位单次 4.894 ms，
/// 是「分包 + FEC 编码」总耗时 6.635 ms 的 **74%**。
///
/// 连续发帧时应当复用同一个编解码器（见 [`packetize_with_codec`]），
/// 让矩阵只建一次、之后每帧只做 GF 乘加。
pub fn packetize(
    frame: &beam_proto::EncodedFrame,
    ratio: f32,
    max_datagram: usize,
    slice_boundary: bool,
) -> Result<Vec<Packet>, NetError> {
    let codec = FecCodec::new();
    packetize_with_codec(frame, ratio, max_datagram, slice_boundary, &codec)
}

/// 同 [`packetize`]，但**复用调用方持有的 FEC 编解码器**。
///
/// `codec` 内部按 `(k, m)` 缓存 `ReedSolomon`（见 [`crate::fec::FecCodec`]），
/// 因此同一个编解码器连续处理多帧时，RS 矩阵只构建一次。
/// 发送路径由 [`crate::endpoint::Conn`] 持有这样一个长生命周期的编解码器。
///
/// 传进来的编解码器可以是任意状态：`(k, m)` 变了会自动新建矩阵。
pub fn packetize_with_codec(
    frame: &beam_proto::EncodedFrame,
    ratio: f32,
    max_datagram: usize,
    slice_boundary: bool,
    codec: &FecCodec,
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
    // 负载前缀（长度 + 总组数）必须放得进第 0 片的第 0 片，
    // 否则接收端读不到「帧在哪里结束」⇒ 直接报错，不发这种帧。
    if shard_len < LEN_PREFIX {
        return Err(NetError::ReassemblyFailed {
            reason: format!(
                "分片长度 {shard_len} 装不下 {LEN_PREFIX} 字节负载前缀(max_datagram 太小)"
            ),
        });
    }
    // 冗余片数：feature 关闭时恒为 0（MVP 不产生冗余分片）。
    let m = effective_parity_shard_count(k, ratio);

    // 构造带长度前缀的填充负载。
    let padded_len = groups * k * shard_len;
    let mut padded = vec![0u8; padded_len];
    padded[..4].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    // 总组数也写进前缀：它是接收端判断「帧在哪里结束」的唯一可靠依据
    // （帧头里带 LAST_SHARD 标志的数据报可能丢，见模块文档）。
    padded[4..8].copy_from_slice(&(groups as u32).to_be_bytes());
    padded[LEN_PREFIX..LEN_PREFIX + payload.len()].copy_from_slice(payload);

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

/// 从组 0 的第 0 片读出帧的总组数（负载长度前缀的第二段）。
///
/// 第 0 片是**数据片**，所以前缀和其它数据片一样受 FEC 保护：
/// 它丢了可以被恢复出来，因此「帧的跨度」不依赖任何单个数据报是否到达。
fn group_count_of(group: &FecGroup) -> Option<usize> {
    let shard = group.shards().first()?.as_ref()?;
    if shard.len() < LEN_PREFIX {
        return None;
    }
    let n = u32::from_be_bytes([shard[4], shard[5], shard[6], shard[7]]) as usize;
    if n == 0 || n > MAX_GROUPS_PER_FRAME {
        return None;
    }
    Some(n)
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
    /// 带 `LAST_SHARD` 标志的数据报所在的组号（帧末组，快路径）。
    last_group: Option<usize>,
    /// 本帧的总组数（来自负载长度前缀，可靠：受 FEC 保护）。
    group_count: Option<usize>,
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

    /// 用**已有的** FEC 编解码器新建重组器。
    ///
    /// 目的：让多个重组器共享同一份 RS 矩阵（以及 `reed-solomon-erasure`
    /// 内部的解码矩阵 LRU 缓存）。克隆 [`FecCodec`] 很便宜 ——
    /// 它只是共享 `Arc<ReedSolomon>`，不会复制矩阵。
    ///
    /// 典型用法：接收端把一个编解码器交给所有重组器，
    /// 于是「解码矩阵求逆」只在整条连接上发生一次，而不是每帧一次。
    pub fn with_codec(codec: FecCodec) -> Self {
        Self {
            codec,
            ..Self::default()
        }
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
                group_count: None,
            });
        if entry.k != k || entry.m != m {
            // 同一帧号出现不同分组参数，说明对端状态错乱，丢弃该帧重新开始。
            self.frames.remove(&frame_id);
            return Ok(None);
        }

        {
            let group = entry
                .groups
                .entry(group_no)
                .or_insert_with(|| FecGroup::new(k, m, payload.len().max(1)));
            // 重复片：直接忽略（§3.2 测试要求「重复检测」）。
            group.insert(local, payload);
        }

        if header.flags.is_last_shard() {
            entry.last_group = Some(group_no);
        }

        // 帧的跨度（总组数）：
        //
        // 优先取负载长度前缀里的组数 —— 它在**数据片**里，受 FEC 保护，
        // 丢了也能恢复出来，所以「帧在哪里结束」不再依赖某一个数据报是否到达；
        // 拿不到时才退回帧头的 `LAST_SHARD` 标志（快路径）。
        if entry.group_count.is_none() && group_no == 0 {
            let count = {
                let group = entry.groups.get_mut(&0).expect("组 0 刚刚插过分片");
                // 组 0 的第 0 片带着前缀；它丢了就靠 FEC 把它恢复出来。
                if group.shards()[0].is_none() && crate::fec::FEC_ENABLED && group.is_recoverable()
                {
                    let _ = self.codec.reconstruct_group(group);
                }
                group_count_of(group)
            };
            if let Some(n) = count {
                entry.group_count = Some(n);
            }
        }

        let frame_groups = match entry.group_count {
            Some(n) => n,
            None => match entry.last_group {
                Some(g) => g + 1,
                // 帧的跨度还未知：等组 0 的第 0 片（或带 `LAST_SHARD` 的数据报）到。
                None => return Ok(None),
            },
        };

        // 尝试恢复并判断是否完整。
        let mut complete = true;
        for g in 0..frame_groups {
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
        for g in 0..frame_groups {
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

    /// 复用编解码器**不得**改变分包的字节结果。
    ///
    /// `packetize_with_codec` 只是把「新建编解码器」换成「复用编解码器」，
    /// 线上格式必须**逐字节一致** —— 这条测试就是防止
    /// 「为了提速而悄悄改了协议」。
    #[test]
    fn shared_codec_produces_identical_packets() {
        let payload: Vec<u8> = (0..9000u32).map(|i| (i % 251) as u8).collect();
        let f = frame(payload);
        let once = packetize(&f, 0.2, 1200, false).expect("应当能分包");

        let codec = FecCodec::new();
        let reused = packetize_with_codec(&f, 0.2, 1200, false, &codec).expect("应当能分包");
        let reused_again = packetize_with_codec(&f, 0.2, 1200, false, &codec).expect("应当能分包");

        assert_eq!(once, reused, "复用编解码器不得改变分包结果");
        assert_eq!(once, reused_again, "连续复用也不得改变分包结果");
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
