//! Reed-Solomon FEC（§5：按帧 Reed-Solomon GF(2^8)，`k ≤ 200`，`m = ceil(k×ratio)`）。
//!
//! §5 规则：
//! - 默认 `ratio = 0.2`；码率 >150 Mbps 降到 `0.1`；丢包 >2% 升到 `0.3`；
//! - 单帧超过 255 片时拆成多个组。
//!
//! 实现使用 `reed-solomon-erasure`（GF(2^8) 擦除码）：
//! 缺失片以 `None` 表示，恢复时按 `k` 片数据即可重建任意 `m` 片冗余。

use std::collections::HashMap;
use std::sync::Arc;

use reed_solomon_erasure::galois_8::Field;
use reed_solomon_erasure::ReedSolomon;

use crate::error::NetError;

/// §5：单个 FEC 组的数据片上限 `k`。
pub const MAX_DATA_SHARDS: usize = 200;

/// §5：单组总分片上限（`k + m ≤ 255`，因为帧头字段是 8 位）。
pub const MAX_SHARDS_PER_GROUP: usize = 255;

/// 计算冗余片数量 `m = ceil(k × ratio)`，并保证 `k + m ≤ 255`。
pub fn parity_shard_count(k: usize, ratio: f32) -> usize {
    if k == 0 {
        return 0;
    }
    let m = (k as f32 * ratio).ceil() as usize;
    m.min(MAX_SHARDS_PER_GROUP.saturating_sub(k))
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
        let m = parity_shard_count(k, ratio);
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

/// FEC 编解码器，缓存 RS 矩阵，避免每帧重建（§3.2 性能预算）。
#[derive(Debug, Default)]
pub struct FecCodec {
    codecs: HashMap<(usize, usize), Arc<ReedSolomon<Field>>>,
}

impl FecCodec {
    /// 新建编解码器。
    pub fn new() -> Self {
        Self::default()
    }

    fn codec(&mut self, k: usize, m: usize) -> Result<Arc<ReedSolomon<Field>>, NetError> {
        if let Some(codec) = self.codecs.get(&(k, m)) {
            return Ok(Arc::clone(codec));
        }
        let codec = ReedSolomon::<Field>::new(k, m).map_err(|e| NetError::ReassemblyFailed {
            reason: format!("创建 RS 编解码器失败(k={k}, m={m}): {e}"),
        })?;
        let codec = Arc::new(codec);
        self.codecs.insert((k, m), Arc::clone(&codec));
        Ok(codec)
    }

    /// 为组生成冗余片。
    pub fn encode_group(&mut self, group: &mut FecGroup) -> Result<(), NetError> {
        if group.m == 0 {
            return Ok(());
        }
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

    /// 恢复缺失片；不可恢复时返回 `E2006`。
    pub fn reconstruct_group(&mut self, group: &mut FecGroup) -> Result<(), NetError> {
        if group.is_complete() {
            return Ok(());
        }
        if !group.is_recoverable() {
            return Err(NetError::FecUnrecoverable {
                missing: group.missing(),
                parity: group.present().saturating_sub(group.k),
            });
        }
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
    fn can_recover_one_lost_shard() {
        let data = shards(10, 64, 1);
        let mut codec = FecCodec::new();
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
        let mut codec = FecCodec::new();
        let mut group = FecGroup::encode(&data, 0.0).expect("应当能建组");
        group.shards[0] = None;
        let err = codec
            .reconstruct_group(&mut group)
            .expect_err("没有冗余时不可恢复");
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
