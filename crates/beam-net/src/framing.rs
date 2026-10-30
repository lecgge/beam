//! 控制 / 输入流的**分帧编解码**（§5 控制 stream 0）。
//!
//! ## 本模块是分帧格式的唯一真值来源
//!
//! §5 规定控制通道是「双向可靠 stream 0」，承载 [`beam_proto::ControlMsg`]；
//! 线上帧格式是 **4 字节大端长度前缀 + postcard 负载**，
//! 负载自带 **1 字节消息族标签**（[`beam_proto::message::family`]，由
//! [`beam_proto::encode`] 产生）。
//!
//! 该约定此前在 `beam-client` 与 `beam-host` **各写了一份**：§4 不允许 L3 横向依赖
//! （`beam-host` 与 `beam-client` 同层），所以当时只能各写一份 —— 但两份实现
//! **必须永远保持字节级一致**，改了一边忘了另一边，线上直接不通且极难发现。
//!
//! **本模块是唯一真值来源**：
//!
//! - `beam-client` 的控制 / 输入收发**已改用**本模块；
//! - `beam-host` 侧那份同名实现**应当删除**，改为调用本模块
//!   （具体改法见本模块末尾「另一侧怎么改」）。
//!
//! ## 线上格式（字节级，**不得更改**）
//!
//! ```text
//! +------------------------+----------------------------------+
//! | 长度前缀（4 字节大端）  | 负载（族标签 1 字节 + postcard）  |
//! +------------------------+----------------------------------+
//! ```
//!
//! - 长度前缀是**负载长度**（不含前缀本身），大端 `u32`；
//! - 负载第 1 字节是消息族标签（控制 = 1、输入 = 2、统计 = 3），
//!   其后是 postcard 编码的消息体；
//! - 单帧负载上限 [`MAX_FRAME_LEN`] = 64 KiB（防御性上限）。
//!
//! 长度前缀是 §5 之外的**实现约定**，但它是**线上事实**：现有两端实现都按此收发，
//! 改格式等于破坏兼容性，因此本模块只做「把现有格式收敛到一处」，**不动字节**。
//!
//! ## 错误码映射
//!
//! 本模块**不引入新的错误码段**（网络层固定 `E2xxx`）。分帧层的全部失败
//! （长度异常、负载解码失败）统一映射到 **E2005**
//! （[`NetError::ReassemblyFailed`]，语义「重组失败」）：
//!
//! - E2005 已在 `E2xxx` 段内，**不需要**新分配码位；
//! - `From<beam_proto::ProtoError> for NetError` 本来就走同一个码位，
//!   因此「负载解码失败」与「长度前缀异常」不会分裂成两个错误码。
//!
//! 调用方（`beam-client` / `beam-host`）再把它转成本层的传输错误，
//! 并保留 [`NetError::code_str`] 以便日志与上报能看出码位。
//!
//! ## 另一侧怎么改（`beam-host`）
//!
//! `crates/beam-host/src/net.rs` 目前**还有一份自己的分帧实现**（同名的
//! `FRAME_LEN_PREFIX` / `MAX_FRAME_LEN` / `FrameDecoder` / `encode_frame`）。
//! 那份**应当删除并改用本模块**，否则两端仍是两份实现、各改各的。
//!
//! 具体改法：
//!
//! 1. **删除**本文件里的四样东西：
//!    `const FRAME_LEN_PREFIX`、`const MAX_FRAME_LEN`、
//!    `struct FrameDecoder`（含 impl）、`fn encode_frame<M: Message>`；
//! 2. **改导入**：`use beam_net::framing::{decode_payload, encode_frame, FrameDecoder};`
//!    （`beam-host` 已依赖 `beam-net`，无需改 `Cargo.toml`）；
//! 3. **`HostTransport::send_control`**：`let frame = encode_frame(&msg)?;` 改成
//!
//!    ```ignore
//!    let frame = encode_frame(&msg).map_err(|e| SessionError::TransportFailed {
//!        reason: format!("编码控制消息失败 {}: {e}", e.code_str()),
//!    })?;
//!    ```
//!
//!    （`beam-net` 的 `encode_frame` 返回 `NetError`，不再是 `SessionError`）；
//! 4. **`HostTransport::take_control_frame`**：`beam_proto::decode_as::<ControlMsg>(&frame)`
//!    换成 `decode_payload::<ControlMsg>(&frame)`，两处 `Err` 分支都映射成
//!    `SessionError::TransportFailed { reason: format!("… {}: {e}", e.code_str()) }`；
//!    `self.ctrl_in.next_frame()` 的 `Err(e)` 分支同样要映射（`NetError` → `SessionError`）；
//! 5. **`HostInput::take_buffered_event`**：`beam_proto::decode_as::<InputEvent>(&frame)`
//!    换成 `decode_payload::<InputEvent>(&frame)`，错误映射同上；
//!    `self.decoder.next_frame()` 的 `Err(e)` 同样要映射；
//! 6. **模块文档**里「线上帧格式」那段改为指向 [`crate::framing`]。
//!
//! 改完后 `beam-host` 里的 `frame_round_trip_through_length_prefix` /
//! `oversized_frame_is_rejected` 两个测试要么删掉，要么改成调用
//! `beam_net::framing` 的同名 API —— 覆盖已由 [`crate::framing`] 的测试承担
//! （那里的测试还额外把线上字节**钉成固定常量**）。

use bytes::BytesMut;

use beam_proto::Message;

use crate::error::NetError;

/// 线上帧长度前缀长度（4 字节大端）。
pub const LENGTH_PREFIX_LEN: usize = 4;

/// 单帧负载上限（防御性上限；控制消息与输入事件都很小）。
pub const MAX_FRAME_LEN: usize = 64 * 1024;

/// 把一条线上消息编码成「4 字节大端长度前缀 + 负载」。
///
/// 负载 = 1 字节消息族标签（[`beam_proto::Message::FAMILY`]）+ postcard 消息体，
/// 由 [`beam_proto::encode`] 产生。
///
/// # 错误
///
/// postcard 编码失败时返回 [`NetError::ReassemblyFailed`]（E2005）。
pub fn encode_frame<M: Message>(msg: &M) -> Result<Vec<u8>, NetError> {
    let mut buf = BytesMut::new();
    // `From<ProtoError> for NetError` 已存在，`?` 直接落到 E2005。
    beam_proto::encode(msg, &mut buf)?;
    let len = u32::try_from(buf.len()).map_err(|_| NetError::ReassemblyFailed {
        reason: format!("线上帧负载过长({} 字节)", buf.len()),
    })?;
    let mut out = Vec::with_capacity(LENGTH_PREFIX_LEN + buf.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&buf);
    Ok(out)
}

/// 解出一条帧**负载**（族标签 + postcard）里的指定类型消息。
///
/// # 错误
///
/// - 族标签不匹配 / postcard 负载非法 → [`NetError::ReassemblyFailed`]（E2005）。
pub fn decode_payload<M: Message>(payload: &[u8]) -> Result<M, NetError> {
    Ok(beam_proto::decode_as::<M>(payload)?)
}

/// 增量分帧器：从字节流里切出完整帧。
///
/// 与写侧 [`encode_frame`] 配对使用；两端的分帧行为必须完全一致，
/// 因此本类型是 `beam-net` 提供的**唯一**分帧实现。
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// 新建一个空的分帧器。
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 把新到达的字节追加进缓冲。
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// 取出一条完整帧的**负载**（不含长度前缀）。
    ///
    /// - 数据不够（半截帧）时返回 `Ok(None)`，已收到的字节留在缓冲里等后续补齐；
    /// - 长度前缀异常（为 0 或超上限）时返回 [`NetError::ReassemblyFailed`]（E2005），
    ///   此时**缓冲不动**（流已失同步，无法再切出可信的帧），后续调用会持续报同一个错，
    ///   由调用方决定是否断链重连。
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, NetError> {
        if self.buf.len() < LENGTH_PREFIX_LEN {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        if len == 0 {
            return Err(NetError::ReassemblyFailed {
                reason: "线上帧长度为 0(非法)".to_string(),
            });
        }
        if u64::from(len) > MAX_FRAME_LEN as u64 {
            return Err(NetError::ReassemblyFailed {
                reason: format!("线上帧长度超限({len} 字节,上限 {MAX_FRAME_LEN} 字节)"),
            });
        }
        // 上面的上界检查保证 len <= 64 KiB，在 32/64 位平台上都能安全转成 usize。
        let len = len as usize;
        if self.buf.len() < LENGTH_PREFIX_LEN + len {
            return Ok(None);
        }
        let frame = self.buf[LENGTH_PREFIX_LEN..LENGTH_PREFIX_LEN + len].to_vec();
        self.buf.drain(..LENGTH_PREFIX_LEN + len);
        Ok(Some(frame))
    }

    /// 取出一条完整帧并解码成指定类型的消息。
    ///
    /// 半截帧返回 `Ok(None)`；解码失败返回 [`NetError::ReassemblyFailed`]（E2005）。
    pub fn next_message<M: Message>(&mut self) -> Result<Option<M>, NetError> {
        match self.next_frame()? {
            Some(payload) => Ok(Some(decode_payload::<M>(&payload)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use beam_proto::{Agreed, CodecKind, ControlMsg, InputEvent, KeyAction};

    /// 把字节序列格式成小写十六进制（便于与线上抓包逐字节比对）。
    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn write_then_read_gives_identical_bytes() {
        let msg = ControlMsg::SetBitrate { bps: 25_000_000 };
        let frame = encode_frame(&msg).expect("编码应当成功");

        let mut decoder = FrameDecoder::new();
        decoder.push(&frame);
        let payload = decoder
            .next_frame()
            .expect("切帧不应当失败")
            .expect("应当切出完整帧");

        // 长度前缀描述的正是负载长度。
        let declared = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert_eq!(declared as usize, payload.len(), "长度前缀必须等于负载长度");
        assert_eq!(declared as usize + LENGTH_PREFIX_LEN, frame.len());

        // 写出去的帧 = 长度前缀 + 切出来的负载，**字节完全一致**。
        let mut rebuilt = Vec::new();
        rebuilt.extend_from_slice(&declared.to_be_bytes());
        rebuilt.extend_from_slice(&payload);
        assert_eq!(rebuilt, frame, "写→读必须字节完全一致");

        // 解码回消息本体。
        let back: ControlMsg = decode_payload(&payload).expect("解码应当成功");
        assert_eq!(back, msg);
    }

    #[test]
    fn multiple_frames_are_read_in_order() {
        let msgs = vec![
            ControlMsg::HelloAck {
                agreed: Agreed {
                    codec: CodecKind::H264,
                    width: 1920,
                    height: 1080,
                    fps: 60,
                    features: 0,
                },
            },
            ControlMsg::RequestIdr,
            ControlMsg::Bye,
        ];
        let mut stream = Vec::new();
        for m in &msgs {
            stream.extend_from_slice(&encode_frame(m).expect("编码应当成功"));
        }

        let mut decoder = FrameDecoder::new();
        decoder.push(&stream);
        for m in &msgs {
            let got: ControlMsg = decoder
                .next_message()
                .expect("解码不应当失败")
                .expect("应当有帧");
            assert_eq!(&got, m);
        }
        assert!(
            decoder.next_frame().expect("解码不应当失败").is_none(),
            "读完所有帧后不应再有帧"
        );
    }

    // ------------------------------------------------------------------
    // 跨端一致性：把线上字节**钉死**成常量。
    //
    // 只做「写→读→相等」的往返自洽**发现不了**「两端同时改错」的情况
    // （两边一起改，反馈测试照样全绿，但线上早已不兼容）。
    // 因此这里断言固定的十六进制常量：任何一端改动格式都会立刻在这里暴露。
    // ------------------------------------------------------------------

    #[test]
    fn control_frame_bytes_are_pinned() {
        // ControlMsg::RequestIdr
        //   族标签 0x01 + postcard 变体序号 0x07(7) → 负载 2 字节
        assert_eq!(
            hex(&encode_frame(&ControlMsg::RequestIdr).expect("编码应当成功")),
            "000000020107",
            "ControlMsg::RequestIdr 的线上字节被改动过"
        );

        // ControlMsg::Bye
        //   族标签 0x01 + postcard 变体序号 0x0d(13) → 负载 2 字节
        assert_eq!(
            hex(&encode_frame(&ControlMsg::Bye).expect("编码应当成功")),
            "00000002010d",
            "ControlMsg::Bye 的线上字节被改动过"
        );

        // ControlMsg::SetBitrate { bps: 25_000_000 }
        //   族标签 0x01 + 变体序号 0x08(8) + bps 的 postcard varint
        assert_eq!(
            hex(&encode_frame(&ControlMsg::SetBitrate { bps: 25_000_000 }).expect("编码应当成功")),
            "000000060108c0f0f50b",
            "ControlMsg::SetBitrate 的线上字节被改动过"
        );
    }

    #[test]
    fn input_frame_bytes_are_pinned() {
        // InputEvent::Key { code: 42, action: Down }
        //   族标签 0x02 + 变体序号 0x02(2) + code varint 0x2a + action 变体序号 0x00
        assert_eq!(
            hex(&encode_frame(&InputEvent::Key {
                code: 42,
                action: KeyAction::Down,
            })
            .expect("编码应当成功")),
            "0000000402022a00",
            "InputEvent::Key 的线上字节被改动过"
        );
    }

    #[test]
    fn zero_length_prefix_is_rejected() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&0u32.to_be_bytes());
        let err = decoder.next_frame().expect_err("0 长度必须失败");
        assert_eq!(err.code(), 2005, "长度异常必须落在 E2xxx 段内的 E2005");
    }

    #[test]
    fn oversized_length_prefix_is_rejected() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&((MAX_FRAME_LEN + 1) as u32).to_be_bytes());
        let err = decoder.next_frame().expect_err("超长帧必须失败");
        assert_eq!(err.code(), 2005, "长度异常必须落在 E2xxx 段内的 E2005");
        assert!(
            err.to_string().contains("超限"),
            "错误必须说明是长度超限: {err}"
        );
    }

    #[test]
    fn half_frame_yields_none_until_complete() {
        let frame = encode_frame(&ControlMsg::Bye).expect("编码应当成功");

        let mut decoder = FrameDecoder::new();
        // 连长度前缀都没收全。
        decoder.push(&frame[..2]);
        assert!(
            decoder.next_frame().expect("半截帧不应当报错").is_none(),
            "长度前缀没收全时不应切出帧"
        );

        // 长度前缀收全、负载没收全。
        decoder.push(&frame[2..frame.len() - 1]);
        assert!(
            decoder.next_frame().expect("半截帧不应当报错").is_none(),
            "负载没收全时不应切出帧"
        );

        // 补齐最后一字节。
        decoder.push(&frame[frame.len() - 1..]);
        let payload = decoder
            .next_frame()
            .expect("解码不应当失败")
            .expect("应当切出完整帧");
        let back: ControlMsg = decode_payload(&payload).expect("解码应当成功");
        assert_eq!(back, ControlMsg::Bye);
    }

    #[test]
    fn undecodable_payload_is_reported_not_swallowed() {
        // 族标签写错（0x7f 不是合法族标签）→ 解码必须失败，而不是静默吞掉。
        let mut decoder = FrameDecoder::new();
        decoder.push(&[0x00, 0x00, 0x00, 0x02, 0x7f, 0x00]);
        let payload = decoder
            .next_frame()
            .expect("切帧不应当失败")
            .expect("应当切出完整帧");
        let err = decode_payload::<ControlMsg>(&payload).expect_err("非法族标签必须失败");
        assert_eq!(err.code(), 2005, "解码失败必须落在 E2005");
    }

    #[test]
    fn frame_at_max_len_is_accepted() {
        // 上限本身是合法的：构造一条正好 MAX_FRAME_LEN 字节的负载应当能切出来。
        let payload = vec![0x01u8; MAX_FRAME_LEN];
        let mut stream = Vec::new();
        stream.extend_from_slice(&(MAX_FRAME_LEN as u32).to_be_bytes());
        stream.extend_from_slice(&payload);

        let mut decoder = FrameDecoder::new();
        decoder.push(&stream);
        let got = decoder
            .next_frame()
            .expect("解码不应当失败")
            .expect("应当切出完整帧");
        assert_eq!(got.len(), MAX_FRAME_LEN);
    }
}
