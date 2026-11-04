//! 数据报分流读点（§5 三通道：视频与音频共用同一条 QUIC 连接的 datagram 通道）。
//!
//! ## 为什么必须有一个「唯一的读点」
//!
//! §5 里视频与音频走**同一条** datagram 通道，靠帧头的 `type` 区分
//! （`0` 视频、`1` 音频、`2` FEC 冗余）。
//!
//! 而 [`crate::Reassembler`] **不看 `type`** —— 它把收到的每一条数据报都当成
//! 画面分片处理（按 `flags` 的组号与 `shard_idx` 归组）。
//! 所以音频数据报一旦被喂进去，它会被当成「一帧画面」组装出来，
//! 负载是**没有 8 字节视频前缀**的裸 Opus 字节 ⇒ 解码器拿到的是一坨垃圾。
//!
//! 帧号也会相撞：音频帧号从 1 开始、画面帧号从 0 开始。
//! （`beam_audio::wire` 用「帧号最高位作号段标志」把两边的号段彻底分开，
//! 从根上避免这个相撞；但**读点仍然必须分流**。）
//!
//! ⇒ 分流必须发生在**唯一读走数据报的地方**。本模块就是那个地方。
//!
//! ## 分流规则
//!
//! | 帧头 `type` | 去向 |
//! |---|---|
//! | `0` 视频 | 画面重组器（[`crate::Reassembler`]）→ 抖动缓冲 → 交出去 |
//! | `2` FEC 冗余 | 画面重组器（冗余片是**画面帧**的一部分） |
//! | `1` 音频 | 交给调用方的回调（`aux`） |
//! | `3`–`15` 保留 | 交给调用方的回调（不污染画面重组器） |
//!
//! ## 为什么住在 `beam-net`（L1）
//!
//! §4 规定 **L3 之间不得横向依赖**（`beam-host` 与 `beam-client` 同层）。
//! 而**主机侧的整链验证入口**（`beam-host` 的 `client_loopback` 示例）与
//! **客户端侧**都需要「按 `type` 分流」—— 两边各写一份分流逻辑，迟早会漂移。
//!
//! 本 crate 是 §5 线上帧格式的**唯一真值来源**（[`crate::framing`]），
//! 分流逻辑天然属于这里：`beam-net` 不认识「音频」这个业务概念，
//! 它只认识帧头的 `type` —— 而那正是协议规定的分流依据。

use beam_proto::frame::{FrameKind, VideoPktHeader, FRAME_HEADER_LEN};

use crate::datagram::Reassembler;
use crate::endpoint::Conn;
use crate::error::NetError;
use crate::jitter::JitterBuffer;
use crate::AssembledFrame;

/// 分流读点：读走数据报，按帧头 `type` 分派。
///
/// - 画面（`type = 0`）与 FEC 冗余（`type = 2`）→ 重组器 + 抖动缓冲；
/// - 音频（`type = 1`）与保留类型 → 调用方给的回调 `aux`。
#[derive(Debug)]
pub struct DemuxStream {
    conn: Conn,
    reassembler: Reassembler,
    jitter: JitterBuffer,
    /// 因超时被丢弃的帧（每个对应一条 `E2004`）。
    expired: Vec<NetError>,
}

impl DemuxStream {
    /// 组装分流读点（与连接共用同一条 datagram 通道）。
    pub fn new(conn: &Conn) -> Self {
        Self {
            conn: conn.clone(),
            reassembler: Reassembler::new(),
            jitter: JitterBuffer::new(conn.config().jitter_depth()),
            expired: Vec::new(),
        }
    }

    /// 重组器（用于统计与超时处理）。
    pub fn reassembler(&self) -> &Reassembler {
        &self.reassembler
    }

    /// 抖动缓冲（§3.2 `jitter_frames`）。
    pub fn jitter(&self) -> &JitterBuffer {
        &self.jitter
    }

    /// 读走**所有**已到达的数据报，并返回经抖动缓冲放行的那一帧画面。
    ///
    /// - `aux`：收到**非画面**数据报（音频 / 保留类型）时调用，
    ///   参数是帧头类型与数据报原文。回调里的错误由回调自己处理
    ///   （本方法不替调用方决定「音频失败要不要打断画面」）；
    /// - 画面数据报先进重组器，完整帧过抖动缓冲后被放行（**最多一帧**）。
    ///
    /// 语义与 [`crate::endpoint::VideoStream::try_next_frame`] 一致：
    /// **不阻塞**，只消费已经到达的数据报；不完整帧保留在重组器里。
    pub fn drain<F>(&mut self, mut aux: F) -> Result<Option<AssembledFrame>, NetError>
    where
        F: FnMut(FrameKind, &[u8]),
    {
        // 1) 清理超时帧（本地单调时钟口径），丢弃的帧记为 E2004。
        for frame_id in self.reassembler.expire_stale() {
            self.expired.push(NetError::FrameExpired { frame_id });
        }
        // 2) 非阻塞地读走已经到达的数据报，按帧头 `type` 分派。
        while let Some(datagram) = self.conn.try_read_datagram()? {
            dispatch_datagram(&mut self.reassembler, &mut self.jitter, &datagram, &mut aux)?;
        }
        // 3) 经抖动缓冲放行一帧。
        Ok(self.jitter.pop(crate::now_us()))
    }

    /// 取走因超时被丢弃的帧（每条都是 `E2004`：`NetError::FrameExpired`）。
    pub fn take_expired(&mut self) -> Vec<NetError> {
        std::mem::take(&mut self.expired)
    }
}

/// 按帧头 `type` 分派**一条**数据报。
///
/// 画面（`type = 0`）与 FEC 冗余（`type = 2`）进重组器；
/// 音频（`type = 1`）与保留类型交给 `aux`。
///
/// 抽成独立函数是为了**能单测**：它不碰连接，只需要重组器与抖动缓冲。
fn dispatch_datagram<F>(
    reassembler: &mut Reassembler,
    jitter: &mut JitterBuffer,
    datagram: &[u8],
    aux: &mut F,
) -> Result<(), NetError>
where
    F: FnMut(FrameKind, &[u8]),
{
    let header = VideoPktHeader::decode(datagram)?;
    match header.kind {
        // 画面数据片与 FEC 冗余片都属于**画面帧** ⇒ 进重组器。
        FrameKind::Video | FrameKind::Fec => {
            if let Some(frame) = reassembler.push(&header, &datagram[FRAME_HEADER_LEN..])? {
                jitter.push(frame, crate::now_us());
            }
        }
        // 音频与保留类型 ⇒ 交给调用方，**绝不**进画面重组器。
        other => aux(other, datagram),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use beam_proto::frame::FrameFlags;

    /// 画面数据报的视频负载前缀：4 字节长度 + 4 字节总组数（§5 的 8 字节前缀）。
    fn video_payload(body: &[u8]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(8 + body.len());
        payload.extend_from_slice(&(body.len() as u32).to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(body);
        payload
    }

    fn datagram(kind: FrameKind, frame_id: u32, payload: &[u8]) -> Vec<u8> {
        VideoPktHeader {
            ver: VideoPktHeader::VER,
            kind,
            flags: FrameFlags::empty(),
            frame_id,
            shard_idx: 0,
            data_shards: 1,
            fec_shards: 0,
            capture_ts: 0,
        }
        .to_datagram(payload)
    }

    #[test]
    fn video_datagram_goes_to_the_reassembler_and_assembles_a_frame() {
        let mut reassembler = Reassembler::new();
        let mut jitter = JitterBuffer::new(0);
        let mut aux_calls = 0usize;

        dispatch_datagram(
            &mut reassembler,
            &mut jitter,
            &datagram(FrameKind::Video, 0, &video_payload(b"hevc-frame")),
            &mut |_kind, _dg| aux_calls += 1,
        )
        .expect("分发不应失败");

        assert_eq!(aux_calls, 0, "画面数据报不得走非画面支路");
        let frame = jitter.pop(crate::now_us()).expect("应当组装出一帧画面");
        assert_eq!(frame.payload.as_ref(), b"hevc-frame");
    }

    #[test]
    fn audio_datagram_never_enters_the_video_reassembler() {
        let mut reassembler = Reassembler::new();
        let mut jitter = JitterBuffer::new(0);
        let mut seen: Vec<(FrameKind, Vec<u8>)> = Vec::new();

        // 裸 Opus 负载（**没有** 8 字节视频前缀）—— 与真实音频数据报同形。
        dispatch_datagram(
            &mut reassembler,
            &mut jitter,
            &datagram(FrameKind::Audio, 1, b"opus-bytes"),
            &mut |kind, dg| seen.push((kind, dg.to_vec())),
        )
        .expect("分发不应失败");

        assert_eq!(seen.len(), 1, "音频数据报必须交给非画面支路");
        assert_eq!(seen[0].0, FrameKind::Audio);
        assert!(
            jitter.pop(crate::now_us()).is_none(),
            "音频数据报绝不能被组装成画面帧"
        );
    }

    #[test]
    fn audio_and_video_with_the_same_frame_id_do_not_collide() {
        // 音频帧号带「号段标记」（最高位 = 1），画面帧号不带 ⇒ 不会相撞。
        let mut reassembler = Reassembler::new();
        let mut jitter = JitterBuffer::new(0);
        let mut aux_seen = 0usize;

        dispatch_datagram(
            &mut reassembler,
            &mut jitter,
            &datagram(FrameKind::Video, 7, &video_payload(b"video")),
            &mut |_k, _d| aux_seen += 1,
        )
        .expect("分发不应失败");
        dispatch_datagram(
            &mut reassembler,
            &mut jitter,
            &datagram(FrameKind::Audio, 7 | 0x8000_0000, b"audio"),
            &mut |_k, _d| aux_seen += 1,
        )
        .expect("分发不应失败");

        assert_eq!(aux_seen, 1);
        let frame = jitter.pop(crate::now_us()).expect("画面帧应当还在");
        assert_eq!(
            frame.payload.as_ref(),
            b"video",
            "画面帧的负载不得被音频分片污染"
        );
    }

    #[test]
    fn reserved_kinds_are_routed_to_the_auxiliary_branch() {
        let mut reassembler = Reassembler::new();
        let mut jitter = JitterBuffer::new(0);
        let mut kinds: Vec<FrameKind> = Vec::new();

        dispatch_datagram(
            &mut reassembler,
            &mut jitter,
            &datagram(FrameKind::Reserved(9), 3, b"??"),
            &mut |kind, _dg| kinds.push(kind),
        )
        .expect("分发不应失败");

        assert_eq!(kinds, vec![FrameKind::Reserved(9)]);
        assert!(jitter.pop(crate::now_us()).is_none());
    }

    #[test]
    fn malformed_datagram_is_reported_not_swallowed() {
        let mut reassembler = Reassembler::new();
        let mut jitter = JitterBuffer::new(0);
        let err = dispatch_datagram(
            &mut reassembler,
            &mut jitter,
            &[0x11, 0x00],
            &mut |_k, _d| {},
        )
        .expect_err("长度不足必须失败");
        assert_eq!(err.code(), 2005, "帧头解析失败必须映射到 E2005");
    }
}
