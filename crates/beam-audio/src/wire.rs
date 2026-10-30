//! 音频数据报的线上格式（§5 帧头 `type = 1`）。
//!
//! ## 为什么音频走**独立路径**（不进 FEC 分组）
//!
//! §5 的帧头把冗余片标成 `type = 2`（FEC 冗余），而这个类型**不携带**
//! 「属于哪一帧的哪种媒体」的信息。接收端因此分不清「音频帧的冗余片」与
//! 「视频帧的冗余片」—— 这是台账里登记的待裁决协议问题
//! （`beam-net` 的模块文档「音频分包：尚未实现（登记项）」记着同一条）。
//!
//! 因此本模块规定：
//!
//! - **一个音频数据报 = 一帧 Opus 负载**：不分片、不进 FEC 组；
//! - 音频数据报**不带**视频路径那个 8 字节负载前缀（负载长度 + 总组数）——
//!   那个前缀是给「等长分片 + RS 冗余」用的，音频单片单帧，用不上；
//! - 音频帧头的 `data_shards = 1`、`fec_shards = 0`，**永远不会**产生
//!   `type = 2` 的冗余片 ⇒ 接收端按 `type` 分流时不存在歧义；
//! - 一帧 5 ms @128 kbps ≈ 80 B，远小于 §5 的 1184 B 负载上限 ⇒ 不需要分包。
//!
//! ## 帧头字段怎么填（§5 的 16 字节帧头**一位都没有改**）
//!
//! | 字段 | 音频帧的取值 | 依据 |
//! |---|---|---|
//! | `ver` | 1 | §5 当前版本 |
//! | `type` | 1（音频） | §5 帧头表 |
//! | `flags` | 0 | 音频没有「关键帧 / 帧末片 / 切片边界」的概念；bit3..bit7 的组号对音频无意义，保持 0 |
//! | `frame_id` | 音频自己的递增号（最高位置 1，见 [`AUDIO_FRAME_ID_FLAG`]） | 与画面帧号**分开编号**，避免两边的帧号在同一个重组器里相撞 |
//! | `shard_idx` | 0 | 音频不分片 |
//! | `data_shards` | 1 | 一个数据报就是一帧 |
//! | `fec_shards` | 0 | 音频不进 FEC 分组 |
//! | `capture_ts` | 主机采集该音频帧的时刻（微秒） | §5；**与画面时钟不同源**，见下 |
//!
//! ## 时钟：音频与画面**不同源**
//!
//! 画面通道的 `capture_ts` 来自抓屏层自己的单调时钟；音频通道的 `capture_ts`
//! 来自音频采集侧自己的时钟。两者的**零点不同、采集时刻不同**，
//! 因此这两个时间戳**不可互相比较**，也不能拿来算「音画同步偏差」。
//! 本模块只负责把主机侧的采集时刻如实写进帧头，不做任何对齐。
//!
//! ## 谁用这个模块
//!
//! - 主机侧（`beam-host`）：[`encode_audio_datagram`] 把一帧 Opus 装成数据报；
//! - 客户端侧（`beam-client`）：[`is_audio_datagram`] 判断数据报归属，
//!   [`decode_audio_datagram`] 解出 Opus 负载。
//!
//! 放在 `beam-audio` 而不是 `beam-proto`，是因为 §4 规定 **L3 之间不得横向依赖**
//! （`beam-host` 与 `beam-client` 同层），而两端必须用**同一份**字节级实现 ——
//! 两边各写一份必然会漂移（`beam_net::framing` 的模块文档记着同样的教训）。

use bytes::Bytes;

use beam_proto::frame::{FrameFlags, FrameKind, VideoPktHeader, FRAME_HEADER_LEN, MAX_PAYLOAD_LEN};

use crate::error::AudioError;

/// 音频数据报的负载上限（字节）。
///
/// §5：16 字节帧头 + 负载 ≤ 1200 B（`max_datagram`）⇒ 负载 ≤ 1184 B。
/// 一帧 5 ms @128 kbps ≈ 80 B，余量充足。
pub const AUDIO_MAX_PAYLOAD_LEN: usize = MAX_PAYLOAD_LEN;

/// 音频帧号的**独立号段**标记（最高位置 1）。
///
/// 画面帧号由编码器给出（从 0 开始递增），音频帧号从 1 开始 —— 两边的号
/// 必然相撞。一旦相撞，误把音频数据报喂给画面重组器的那条路径
/// （见 `beam-net` 的 `Reassembler`）就会把音频分片当成画面分片拼进去。
/// 用最高位把两边的号段彻底分开，从根上避免这种相撞。
pub const AUDIO_FRAME_ID_FLAG: u32 = 0x8000_0000;

/// 音频帧号序号的有效位宽（去掉号段标记后剩下的 31 位）。
const AUDIO_SEQ_MASK: u32 = 0x7FFF_FFFF;

/// 一条音频数据报解出来的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDatagram {
    /// 帧号（带 [`AUDIO_FRAME_ID_FLAG`] 标记；用 [`AudioDatagram::sequence`] 取序号）。
    pub frame_id: u32,
    /// 主机采集该帧的时刻（微秒）。**与画面时钟不同源**。
    pub capture_ts_us: u64,
    /// Opus 负载（一帧 5 ms）。
    pub payload: Bytes,
}

impl AudioDatagram {
    /// 帧号里的序号部分（去掉独立号段标记）。
    pub fn sequence(&self) -> u32 {
        self.frame_id & AUDIO_SEQ_MASK
    }
}

/// 把序号变成音频帧号（最高位置 1 ⇒ 与画面帧号不同号段）。
pub fn audio_frame_id(sequence: u32) -> u32 {
    AUDIO_FRAME_ID_FLAG | (sequence & AUDIO_SEQ_MASK)
}

/// 构造一条音频数据报（帧头 + Opus 负载）。
///
/// - `sequence`：音频帧的递增序号（0 起）；帧号由 [`audio_frame_id`] 生成，
///   与画面帧号不同号段；
/// - `capture_ts_us`：主机采集该帧的时刻（微秒）；
/// - `opus`：一帧 Opus 负载（5 ms）。
///
/// 负载超过 [`AUDIO_MAX_PAYLOAD_LEN`] ⇒ `E7005`（数据报非法）：
/// 音频**不分片**，超长只能报错，不静默截断。
pub fn encode_audio_datagram(
    sequence: u32,
    capture_ts_us: u64,
    opus: &[u8],
) -> Result<Vec<u8>, AudioError> {
    if opus.len() > AUDIO_MAX_PAYLOAD_LEN {
        return Err(AudioError::DatagramInvalid {
            reason: format!(
                "Opus 负载 {} 字节超过音频数据报上限 {AUDIO_MAX_PAYLOAD_LEN} 字节(音频不分片)",
                opus.len()
            ),
        });
    }
    let header = VideoPktHeader {
        ver: VideoPktHeader::VER,
        kind: FrameKind::Audio,
        // 音频没有关键帧 / 帧末片 / 切片边界的概念；
        // bit3..bit7 的「FEC 组号」对单片音频无意义，保持 0。
        flags: FrameFlags::empty(),
        frame_id: audio_frame_id(sequence),
        // 音频不分片：组内分片号恒为 0。
        shard_idx: 0,
        // 一个数据报就是一帧，且**不进 FEC 分组**。
        data_shards: 1,
        fec_shards: 0,
        capture_ts: capture_ts_us,
    };
    Ok(header.to_datagram(opus))
}

/// 解析一条音频数据报。
///
/// - 帧头不合法 / 类型不是音频 / 分片参数不是「1 片 0 冗余」⇒ `E7005`；
/// - 返回的 `payload` 是**原始 Opus 负载**（没有视频路径那个 8 字节前缀）。
pub fn decode_audio_datagram(datagram: &[u8]) -> Result<AudioDatagram, AudioError> {
    let header = VideoPktHeader::decode(datagram).map_err(|e| AudioError::DatagramInvalid {
        reason: format!("帧头解析失败: {e}"),
    })?;
    if !matches!(header.kind, FrameKind::Audio) {
        return Err(AudioError::DatagramInvalid {
            reason: format!("帧头类型不是音频(实际 type = {})", header.kind.bits()),
        });
    }
    if header.data_shards != 1 || header.fec_shards != 0 {
        return Err(AudioError::DatagramInvalid {
            reason: format!(
                "音频帧必须是 1 片 0 冗余(实际 data_shards = {}, fec_shards = {})",
                header.data_shards, header.fec_shards
            ),
        });
    }
    Ok(AudioDatagram {
        frame_id: header.frame_id,
        capture_ts_us: header.capture_ts,
        payload: Bytes::copy_from_slice(&datagram[FRAME_HEADER_LEN..]),
    })
}

/// 判断一个数据报是不是音频（只看帧头的 `type` 低 4 位）。
///
/// 长度不足帧头时返回 `false`（交给调用方按「非法数据报」处理）。
pub fn is_audio_datagram(datagram: &[u8]) -> bool {
    if datagram.len() < FRAME_HEADER_LEN {
        return false;
    }
    matches!(FrameKind::from_bits(datagram[0] & 0x0F), FrameKind::Audio)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn audio_datagram_header_layout_is_pinned() {
        // 帧头 16 字节大端：ver=1 / type=1（音频）⇒ 第一个字节 0x11。
        let dg = encode_audio_datagram(1, 0x0001_0203_0405, b"opus-payload").expect("编码应当成功");
        assert_eq!(dg.len(), FRAME_HEADER_LEN + b"opus-payload".len());
        assert_eq!(dg[0], 0x11, "ver=1、type=1（音频）⇒ 0b0001_0001");
        assert_eq!(dg[1], 0x00, "音频帧没有关键帧 / 帧末片 / 切片边界 / 组号");
        // frame_id：音频独立号段（最高位置 1）。
        assert_eq!(
            &dg[2..6],
            &audio_frame_id(1).to_be_bytes(),
            "音频帧号必须落在独立号段"
        );
        assert_eq!(&dg[6..8], &[0x00, 0x00], "shard_idx 恒为 0（音频不分片）");
        assert_eq!(dg[8], 1, "data_shards = 1");
        assert_eq!(dg[9], 0, "fec_shards = 0（音频不进 FEC 分组）");
        // capture_ts：48 位大端。
        assert_eq!(&dg[10..16], &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05]);
        // 负载就是 Opus 字节，**没有** 8 字节前缀。
        assert_eq!(&dg[FRAME_HEADER_LEN..], b"opus-payload");
        assert_eq!(
            hex(&dg[..FRAME_HEADER_LEN]),
            "11008000000100000100000102030405",
            "音频数据报的帧头字节被改动过"
        );
    }

    #[test]
    fn round_trip_restores_the_opus_payload() {
        let dg = encode_audio_datagram(7, 123_456, b"opus").expect("编码应当成功");
        let back = decode_audio_datagram(&dg).expect("解码应当成功");
        assert_eq!(back.sequence(), 7);
        assert_eq!(back.capture_ts_us, 123_456);
        assert_eq!(back.payload.as_ref(), b"opus");
    }

    #[test]
    fn audio_frame_ids_never_collide_with_video_frame_ids() {
        // 画面帧号从 0 开始递增；音频帧号最高位是 1 ⇒ 两边永不相撞。
        for seq in 0..1000u32 {
            let audio = audio_frame_id(seq);
            assert_ne!(audio & AUDIO_FRAME_ID_FLAG, 0, "音频帧号必须在独立号段");
            assert_eq!(audio & AUDIO_SEQ_MASK, seq & AUDIO_SEQ_MASK);
        }
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let big = vec![0u8; AUDIO_MAX_PAYLOAD_LEN + 1];
        let err = encode_audio_datagram(0, 0, &big).expect_err("超长必须失败");
        assert_eq!(err.code(), 7005);
        // 正好等于上限是合法的。
        let ok = vec![0u8; AUDIO_MAX_PAYLOAD_LEN];
        assert!(encode_audio_datagram(0, 0, &ok).is_ok());
    }

    #[test]
    fn video_datagram_is_not_classified_as_audio() {
        // 画面帧头：ver=1 / type=0 ⇒ 第一个字节 0x10。
        let mut dg = vec![0u8; FRAME_HEADER_LEN];
        dg[0] = 0x10;
        assert!(!is_audio_datagram(&dg));
        let audio = encode_audio_datagram(0, 0, b"x").expect("编码应当成功");
        assert!(is_audio_datagram(&audio));
    }

    #[test]
    fn short_datagram_is_not_audio_and_fails_to_decode() {
        assert!(!is_audio_datagram(&[0x11, 0x00]));
        let err = decode_audio_datagram(&[0x11, 0x00]).expect_err("长度不足必须失败");
        assert_eq!(err.code(), 7005);
    }

    #[test]
    fn decoding_a_video_datagram_is_rejected() {
        let mut dg = vec![0u8; FRAME_HEADER_LEN];
        dg[0] = 0x10;
        let err = decode_audio_datagram(&dg).expect_err("画面数据报必须被拒绝");
        assert_eq!(err.code(), 7005);
    }
}
