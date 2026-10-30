//! Reed-Solomon FEC（§5：按帧 Reed-Solomon GF(2^8)，`k ≤ 200`，`m = ceil(k×ratio)`）。
//!
//! §5 规则：
//! - 默认 `ratio = 0.2`；码率 >150 Mbps 降到 `0.1`；丢包 >2% 升到 `0.3`；
//! - 单帧超过 255 片时拆成多个组。
//!
//! 实现使用 `reed-solomon-erasure`（GF(2^8) 擦除码）：
//! 缺失片以 `None` 表示，恢复时按 `k` 片数据即可重建任意 `m` 片冗余。
//!
//! ## feature 门控：`fec`
//!
//! | 构建 | feature | 行为 |
//! |---|---|---|
//! | MVP | 默认（`default = []`） | **不产生冗余分片**：`m` 恒为 0，帧头 `fec_shards` 恒为 0；接收端不做 FEC 恢复 |
//! | v1 | `--features fec` | 按 `ratio` 生成 `m = ceil(k×ratio)` 片冗余，接收端用冗余恢复丢片 |
//!
//! - feature 关闭时 `reed-solomon-erasure` 整个依赖都不参与编译（`optional = true` + `dep:` 语法），
//!   因此 MVP 构建里没有 RS 编解码代码，也没有那 20% 的冗余带宽；
//! - [`parity_shard_count`] 是**纯数学函数**（`m = ceil(k×ratio)`），两种构建下都可算、可测；
//!   真正决定「发不发冗余片」的是 [`effective_parity_shard_count`]；
//! - [`FEC_ENABLED`] 是当前构建的编译期常量，运行期不可切换。
//!
//! 两端必须同时打开或同时关闭 `fec`：feature 关闭的接收端拿到带冗余的帧也恢复不了丢片。

#[cfg(feature = "fec")]
use std::collections::HashMap;
#[cfg(feature = "fec")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "fec")]
use reed_solomon_erasure::galois_8::Field;
#[cfg(feature = "fec")]
use reed_solomon_erasure::ReedSolomon;

use crate::error::NetError;

/// §5：单个 FEC 组的数据片上限 `k`。
pub const MAX_DATA_SHARDS: usize = 200;

/// §5：单组总分片上限（`k + m ≤ 255`，因为帧头字段是 8 位）。
pub const MAX_SHARDS_PER_GROUP: usize = 255;

/// 当前构建是否启用 FEC 冗余（编译期常量）。
///
/// - `false`：MVP 构建（默认），不产生冗余分片；
/// - `true`：v1 构建（`--features fec`）。
pub const FEC_ENABLED: bool = cfg!(feature = "fec");

/// 计算冗余片数量 `m = ceil(k × ratio)`，并保证 `k + m ≤ 255`。
///
/// 这是**纯数学函数**，不受 `fec` feature 影响：
/// 即使 MVP 构建（不产生冗余片），也能算出「若开启 FEC 会有几片冗余」。
/// 真正决定发不发冗余片的是 [`effective_parity_shard_count`]。
pub fn parity_shard_count(k: usize, ratio: f32) -> usize {
    if k == 0 {
        return 0;
    }
    let m = (k as f32 * ratio).ceil() as usize;
    m.min(MAX_SHARDS_PER_GROUP.saturating_sub(k))
}

/// 当前构建下真正生效的冗余片数量。
///
/// - `fec` feature 打开：等价于 [`parity_shard_count`]；
/// - `fec` feature 关闭（MVP）：恒为 `0` —— 发送端不产生任何冗余分片，
///   §5 默认 `ratio = 0.2` 的那 20% 带宽在 MVP 阶段被省掉。
pub fn effective_parity_shard_count(k: usize, ratio: f32) -> usize {
    if FEC_ENABLED {
        parity_shard_count(k, ratio)
    } else {
        0
    }
}

/// 一个 FEC 组（同一帧的若干数据片 + 冗余片）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FecGroup {
    /// 数据片数 k。
    pub k: usize,
    /// 冗余片数 m。
    pub m: usize,
    /// 每片字节数（RS 要求等长）。
    pub shard_len: usize,
    /// 共 `k + m` 片；`None` 表示缺失。
    shards: Vec<Option<Vec<u8>>>,
}

impl FecGroup {
    /// 建立一个空组。
    pub fn new(k: usize, m: usize, shard_len: usize) -> Self {
        Self {
            k,
            m,
            shard_len,
            shards: vec![None; k + m],
        }
    }

    /// 用数据片建组并生成 `m` 片冗余（`ratio` 决定 `m`）。
    ///
    /// 冗余片数量取 [`effective_parity_shard_count`]：
    /// `fec` feature 关闭时 `m = 0`，本方法只登记数据片，不做任何 RS 编码。
    pub fn encode(data: &[Vec<u8>], ratio: f32) -> Result<Self, NetError> {
        if data.is_empty() {
            return Err(NetError::ReassemblyFailed {
                reason: "FEC 组没有数据片".to_string(),
            });
        }
        let k = data.len();
        if k > MAX_DATA_SHARDS {
            return Err(NetError::ReassemblyFailed {
                reason: format!("FEC 组数据片 {k} 超过上限 {MAX_DATA_SHARDS}"),
            });
        }
        let m = effective_parity_shard_count(k, ratio);
        let shard_len = data[0].len();
        if data.iter().any(|d| d.len() != shard_len) {
            return Err(NetError::ReassemblyFailed {
                reason: "FEC 组数据片长度不一致".to_string(),
            });
        }

        let mut group = Self::new(k, m, shard_len);
        for (i, d) in data.iter().enumerate() {
            group.shards[i] = Some(d.clone());
        }
        Ok(group)
    }

    /// 插入一片；返回 `false` 表示重复或越界（重复检测）。
    pub fn insert(&mut self, index: usize, bytes: &[u8]) -> bool {
        if index >= self.shards.len() || bytes.len() != self.shard_len {
            return false;
        }
        if self.shards[index].is_some() {
            return false;
        }
        self.shards[index] = Some(bytes.to_vec());
        true
    }

    /// 已收到的片数（含冗余）。
    pub fn present(&self) -> usize {
        self.shards.iter().filter(|s| s.is_some()).count()
    }

    /// 缺失的数据片数。
    pub fn missing_data(&self) -> usize {
        self.shards[..self.k].iter().filter(|s| s.is_none()).count()
    }

    /// 数据片是否已经齐全。
    pub fn is_complete(&self) -> bool {
        self.missing_data() == 0
    }

    /// 是否具备恢复条件（收到片数 ≥ k）。
    pub fn is_recoverable(&self) -> bool {
        self.present() >= self.k
    }

    /// 数据片（缺失的返回空切片）。
    pub fn data_shards(&self) -> Vec<&[u8]> {
        self.shards[..self.k]
            .iter()
            .map(|s| s.as_deref().unwrap_or(&[]))
            .collect()
    }

    /// 全部片（数据 + 冗余）。
    pub fn shards(&self) -> &[Option<Vec<u8>>] {
        &self.shards
    }

    /// 缺失片数（数据 + 冗余）。
    pub fn missing(&self) -> usize {
        self.shards.iter().filter(|s| s.is_none()).count()
    }
}

/// RS 编解码器缓存表：`(k, m)` → RS 编解码器。
///
/// 放在 `Arc<Mutex<..>>` 里 ⇒ 克隆 `FecCodec` 时**共享同一张表**，
/// 而不是复制一份空表（复制空表等于缓存没开）。
#[cfg(feature = "fec")]
type CodecCache = Arc<Mutex<HashMap<(usize, usize), Arc<ReedSolomon<Field>>>>>;

/// FEC 编解码器，缓存 RS 矩阵，避免每帧重建（§3.2 性能预算）。
///
/// `fec` feature 关闭时内部不持有任何 RS 矩阵，编解码方法直接返回错误。
///
/// ## 为什么要复用同一个实例
///
/// `ReedSolomon::new(k, m)` 要在 GF(2⁸) 上求逆一个 `k×k` 矩阵（O(k³)），
/// 实测 T1 档位（`k = 132`）单次 **4.894 ms**；而同一个实例内部的
/// `reed-solomon-erasure` 还带一个解码矩阵 LRU 缓存，
/// 复用实例能让**编码矩阵与解码矩阵都只算一次**。
///
/// ## 为什么缓存放在 `Arc<Mutex<..>>` 里
///
/// 克隆 `FecCodec` 时必须**共享同一份缓存**，而不是复制一份空表：
/// 复制空表会让每个克隆体各建各的矩阵，缓存等于没开。
/// 因此这里用 `Arc<Mutex<HashMap<..>>>`，
/// 克隆只是多一个指向同一张表的句柄。
///
/// 用法：
/// - 发送路径挂在 [`crate::endpoint::Conn`] 上（跨帧复用）；
/// - 接收路径挂在 [`crate::datagram::Reassembler`] 上（跨帧复用），
///   也可以用 [`crate::datagram::Reassembler::with_codec`] 与别的重组器共享。
#[derive(Debug, Default, Clone)]
pub struct FecCodec {
    /// `(k, m)` → RS 编解码器（共享缓存，见类型文档）。
    #[cfg(feature = "fec")]
    codecs: CodecCache,
}

impl FecCodec {
    /// 新建编解码器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 缓存里当前有多少个 RS 编解码器。
    ///
    /// 用来在测试里钉住「矩阵只建一次」这条性能约束：
    /// 连续处理 N 帧之后，这个数应当仍然是 1（同一个 `(k, m)`）。
    /// `fec` feature 关闭时恒为 0（根本不会建矩阵）。
    pub fn cached_codec_count(&self) -> usize {
        #[cfg(feature = "fec")]
        {
            self.codecs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len()
        }
        #[cfg(not(feature = "fec"))]
        {
            0
        }
    }

    /// 为组生成冗余片。
    ///
    /// `m = 0`（feature 关闭，或 `ratio = 0`）时什么都不做。
    ///
    /// 只借用 `&self`：RS 实例在内部缓存里，`FecCodec` 本身是可共享的。
    pub fn encode_group(&self, group: &mut FecGroup) -> Result<(), NetError> {
        if group.m == 0 {
            return Ok(());
        }
        self.encode_group_rs(group)
    }

    /// 恢复缺失片；不可恢复时返回 `E2006`。
    pub fn reconstruct_group(&self, group: &mut FecGroup) -> Result<(), NetError> {
        if group.is_complete() {
            return Ok(());
        }
        if !group.is_recoverable() {
            return Err(NetError::FecUnrecoverable {
                missing: group.missing(),
                parity: group.present().saturating_sub(group.k),
            });
        }
        self.reconstruct_group_rs(group)
    }

    /// 真正的 RS 编码（仅 `fec` feature 打开时存在）。
    #[cfg(feature = "fec")]
    fn encode_group_rs(&self, group: &mut FecGroup) -> Result<(), NetError> {
        let codec = self.codec(group.k, group.m)?;
        let mut flat: Vec<Vec<u8>> = Vec::with_capacity(group.k + group.m);
        for i in 0..group.k {
            flat.push(
                group.shards[i]
                    .clone()
                    .ok_or_else(|| NetError::ReassemblyFailed {
                        reason: format!("FEC 组第 {i} 片缺失,无法生成冗余"),
                    })?,
            );
        }
        for _ in 0..group.m {
            flat.push(vec![0u8; group.shard_len]);
        }
        codec
            .encode(&mut flat)
            .map_err(|e| NetError::ReassemblyFailed {
                reason: format!("FEC 编码失败: {e}"),
            })?;
        for j in 0..group.m {
            group.shards[group.k + j] = Some(flat[group.k + j].clone());
        }
        Ok(())
    }

    /// `fec` feature 关闭时的占位：无法生成冗余片。
    #[cfg(not(feature = "fec"))]
    fn encode_group_rs(&self, group: &mut FecGroup) -> Result<(), NetError> {
        Err(NetError::ReassemblyFailed {
            reason: format!("当前构建未启用 `fec` feature,无法生成 {} 片冗余", group.m),
        })
    }

    /// 真正的 RS 恢复（仅 `fec` feature 打开时存在）。
    #[cfg(feature = "fec")]
    fn reconstruct_group_rs(&self, group: &mut FecGroup) -> Result<(), NetError> {
        let codec = self.codec(group.k, group.m)?;
        let mut flat: Vec<Option<Vec<u8>>> = group.shards.clone();
        codec
            .reconstruct(&mut flat)
            .map_err(|e| NetError::ReassemblyFailed {
                reason: format!("FEC 恢复失败: {e}"),
            })?;
        group.shards = flat;
        Ok(())
    }

    /// `fec` feature 关闭时的占位：无法恢复丢片。
    #[cfg(not(feature = "fec"))]
    fn reconstruct_group_rs(&self, group: &mut FecGroup) -> Result<(), NetError> {
        Err(NetError::FecUnrecoverable {
            missing: group.missing(),
            parity: group.m,
        })
    }

    /// 取（并缓存）`k + m` 的 RS 编解码器。
    ///
    /// 缓存表在 `Arc<Mutex<..>>` 里 ⇒ **克隆体共享同一张表**，
    /// 命中时不会重新求逆矩阵。
    #[cfg(feature = "fec")]
    fn codec(&self, k: usize, m: usize) -> Result<Arc<ReedSolomon<Field>>, NetError> {
        let mut cache = self
            .codecs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(codec) = cache.get(&(k, m)) {
            return Ok(Arc::clone(codec));
        }
        let codec = ReedSolomon::<Field>::new(k, m).map_err(|e| NetError::ReassemblyFailed {
            reason: format!("创建 RS 编解码器失败(k={k}, m={m}): {e}"),
        })?;
        let codec = Arc::new(codec);
        cache.insert((k, m), Arc::clone(&codec));
        Ok(codec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shards(n: usize, len: usize, seed: u8) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                (0..len)
                    .map(|j| {
                        (i as u8)
                            .wrapping_mul(31)
                            .wrapping_add(j as u8)
                            .wrapping_add(seed)
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn parity_count_follows_ceil_rule() {
        assert_eq!(parity_shard_count(10, 0.2), 2);
        assert_eq!(parity_shard_count(11, 0.2), 3);
        assert_eq!(parity_shard_count(1, 0.2), 1);
        assert_eq!(parity_shard_count(200, 0.2), 40);
        assert_eq!(parity_shard_count(200, 0.0), 0);
    }

    #[test]
    fn parity_count_never_exceeds_255_total() {
        assert!(parity_shard_count(200, 1.0) <= 55);
        assert_eq!(parity_shard_count(200, 1.0) + 200, 255);
    }

    #[test]
    fn effective_parity_follows_the_fec_feature() {
        if FEC_ENABLED {
            assert_eq!(effective_parity_shard_count(10, 0.2), 2);
        } else {
            // MVP：配置项保留但不生效，不产生任何冗余片。
            assert_eq!(effective_parity_shard_count(10, 0.2), 0);
            assert_eq!(effective_parity_shard_count(200, 0.2), 0);
        }
    }

    #[test]
    fn group_encodes_without_redundancy_when_feature_is_off() {
        let data = shards(4, 32, 7);
        let group = FecGroup::encode(&data, 0.2).expect("应当能建组");
        assert_eq!(group.k, 4);
        assert_eq!(group.m, effective_parity_shard_count(4, 0.2));
        if !FEC_ENABLED {
            assert_eq!(group.m, 0, "MVP 构建不应产生冗余片");
            assert_eq!(group.shards().len(), 4);
        }
    }

    /// §3.2 性能预算的回归钉子：连续编码多组，RS 矩阵只能建一次。
    ///
    /// `ReedSolomon::new(k, m)` 要在 GF(2⁸) 上求逆一个 `k×k` 矩阵，
    /// 实测 T1 档位单次 4.894 ms —— 如果每帧都重建，预算必然超标。
    #[test]
    #[cfg(feature = "fec")]
    fn repeated_encoding_reuses_the_rs_matrix() {
        let data = shards(10, 64, 1);
        let codec = FecCodec::new();
        for _ in 0..8 {
            let mut group = FecGroup::encode(&data, 0.2).expect("应当能建组");
            codec.encode_group(&mut group).expect("应当能编码");
        }
        assert_eq!(
            codec.cached_codec_count(),
            1,
            "8 次编码只应建 1 个 RS 矩阵（同一个 (k, m)）"
        );
    }

    /// 克隆体必须与原体**共享**缓存表，而不是复制一份空表。
    ///
    /// 复制空表 ⇒ 每个克隆体各建各的矩阵 ⇒ 缓存等于没开
    /// （这正是 B9 批次之前的实现方式）。
    #[test]
    #[cfg(feature = "fec")]
    fn cloned_codec_shares_the_matrix_cache() {
        let codec = FecCodec::new();
        let clone = codec.clone();
        assert!(
            Arc::ptr_eq(&codec.codecs, &clone.codecs),
            "克隆体必须共享同一张缓存表"
        );
    }

    #[test]
    #[cfg(feature = "fec")]
    fn can_recover_one_lost_shard() {
        let data = shards(10, 64, 1);
        let codec = FecCodec::new();
        let mut group = FecGroup::encode(&data, 0.2).expect("应当能建组");
        codec.encode_group(&mut group).expect("应当能编码");
        assert_eq!(group.shards().len(), 12);

        // 丢掉 2 片数据片。
        group.shards[3] = None;
        group.shards[7] = None;
        assert!(!group.is_complete());
        codec.reconstruct_group(&mut group).expect("应当能恢复");
        assert!(group.is_complete());
        for (i, d) in data.iter().enumerate() {
            assert_eq!(group.data_shards()[i], d.as_slice());
        }
    }

    #[test]
    fn unrecoverable_group_reports_e2006() {
        let data = shards(4, 32, 2);
        let codec = FecCodec::new();
        let mut group = FecGroup::encode(&data, 0.0).expect("应当能建组");
        group.shards[0] = None;
        let err = codec
            .reconstruct_group(&mut group)
            .expect_err("没有冗余时不可恢复");
        assert_eq!(err.code(), 2006);
    }

    #[test]
    #[cfg(not(feature = "fec"))]
    fn feature_off_cannot_reconstruct() {
        // MVP 构建：即使收到了足够多的片，也没有 RS 代码可用，必须报 E2006。
        let data = shards(4, 32, 5);
        let codec = FecCodec::new();
        let mut group = FecGroup::encode(&data, 0.2).expect("应当能建组");
        group.shards[0] = None;
        let err = codec
            .reconstruct_group(&mut group)
            .expect_err("未启用 FEC 时不可恢复");
        assert_eq!(err.code(), 2006);
    }

    #[test]
    fn duplicate_insert_is_rejected() {
        let data = shards(3, 16, 3);
        // 用空组测试插入语义（`encode` 之后数据片已就位）。
        let mut group = FecGroup::new(3, 1, 16);
        assert!(group.insert(0, &data[0]));
        assert!(!group.insert(0, &data[0]), "重复片必须被识别");
        assert!(!group.insert(99, &data[0]), "越界索引必须被拒绝");
        assert!(!group.insert(1, &data[1][..8]), "长度不符必须被拒绝");
    }
}
