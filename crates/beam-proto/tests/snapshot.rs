//! §3.1 测试要求：版本兼容快照。
//!
//! §5 要求「次版本向后兼容，未知字段忽略，postcard 末尾追加」。
//! 本文件把**线上字节序列**固化成十六进制字面量常量：
//! 任何对 postcard 编码结果、枚举变体顺序、帧头位宽的改动，都会让这些断言立刻失败，
//! 从而防止「无意改动线上格式」——线上格式一旦漂移，两端就再也互不兼容。
//!
//! 这里刻意不引入 `insta` 等快照框架：十六进制字面量本身就是快照，
//! 且不增加任何依赖面，评审时一眼能看出线上字节是什么。
//!
//! ## 覆盖范围
//!
//! - 一条 `ControlMsg`（`Hello`）的完整线上字节（含 1 字节消息族标签）；
//! - 16 字节帧头的逐位字节；
//! - `Hello` / `HelloAck` / `PairOk` 三条关键消息；
//! - 前向兼容：给旧版本消息尾部追加未知字节后仍能解回。
//!
//! ## 前向兼容的边界（务必先读）
//!
//! postcard 的结构体编码是**按字段声明顺序位置化**的，没有字段名，
//! 所以「新增字段」只有在**整条消息的最末尾**才是安全的：
//!
//! - `Hello` 的最后一个字段是 `caps`，`Caps` 的最后一个字段是 `features`；
//!   因此给 `Caps` 追加字段时，新字节正好落在整条消息的尾部 → 旧解码器可忽略。
//! - 反之，若给 `Hello` 追加一个位于 `caps` **之后**的字段，再给 `Caps` 追加字段，
//!   新字节就会插到消息**中间**，旧解码器会把新字段的前几个字节当成
//!   `Caps.features` 继续解析 → 静默错解，而不是报错。
//!
//! 也就是说：§5 的「未知字段忽略」只在**尾部追加**这一种形态下成立，
//! 改动消息结构时必须把新字段加在最后一个结构体的末尾。

use beam_proto::caps::{feature, Agreed, Caps, CodecKind, ProtocolVersion, Resolution};
use beam_proto::frame::{FrameFlags, FrameKind, VideoPktHeader, FRAME_HEADER_LEN};
use beam_proto::message::{ControlMsg, Hello};
use beam_proto::{decode, decode_as, encode};
use bytes::BytesMut;

/// 把字节渲染成 `0x01 0x02` 形式的十六进制串，便于断言与排错。
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 快照用的 `Hello` 能力集（与 §8 T0 档位一致）。
fn snapshot_caps() -> Caps {
    Caps {
        codecs: vec![CodecKind::H264, CodecKind::Hevc],
        max_res: Resolution::new(1920, 1080),
        max_fps: 60,
        features: feature::AUDIO,
    }
}

/// 快照用的 `Hello` 控制消息。
fn snapshot_hello() -> ControlMsg {
    ControlMsg::Hello(Hello {
        ver: ProtocolVersion::CURRENT,
        caps: snapshot_caps(),
    })
}

/// `Hello` 控制消息的完整线上字节（1 字节消息族标签 + postcard 负载）。
///
/// 结构：族标签 `01` / 变体序号 `00`(=Hello) / ver.major `01` / ver.minor `00` /
/// codecs 长度 `02` / H264 `00` / Hevc `01` / width(1920 变长) /
/// height(1080 变长) / max_fps(60 变长) / features(1 变长)。
const HELLO_BYTES: &str = "01 00 01 00 02 00 01 80 0f b8 08 3c 01";

/// `HelloAck` 控制消息的完整线上字节。
const HELLO_ACK_BYTES: &str = "01 01 00 80 0f b8 08 3c 01";

/// `PairOk` 控制消息的完整线上字节（族标签 + 变体序号 + 32 字节指纹）。
const PAIR_OK_BYTES: &str = "01 04 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5 a5";

/// 16 字节帧头的逐位快照：ver|type / flags / frame_id / shard_idx / k / m / capture_ts。
const FRAME_HEADER_BYTES: &str = "10 01 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e";

/// 快照用的帧头（各字段取值互不相同，便于逐位核对）。
fn snapshot_header() -> VideoPktHeader {
    VideoPktHeader {
        ver: VideoPktHeader::VER,
        kind: FrameKind::Video,
        flags: FrameFlags::empty().with(FrameFlags::KEYFRAME),
        frame_id: 0x0102_0304,
        shard_idx: 0x0506,
        data_shards: 0x07,
        fec_shards: 0x08,
        capture_ts: 0x090A_0B0C_0D0E,
    }
}

/// 编码一条控制消息，返回其线上字节。
fn encode_bytes(msg: &ControlMsg) -> Vec<u8> {
    let mut buf = BytesMut::new();
    encode(msg, &mut buf).expect("编码应当成功");
    buf.to_vec()
}

/// 一条 `ControlMsg` 的 postcard 编码结果被固定下来。
///
/// 这里特意用 `SetBitrate` 而不是 `Hello`，以便额外覆盖
/// **多字节变长整数**（postcard 的 LEB128 变长编码）的线上形态。
///
/// 结构：族标签 `01` / 变体序号 `08`(=SetBitrate) /
/// bps(25_000_000 变长) = `c0 f0 f5 0b`。
const SET_BITRATE_BYTES: &str = "01 08 c0 f0 f5 0b";

/// 一条 `ControlMsg` 的 postcard 编码结果被固定下来。
#[test]
fn control_msg_bytes_are_frozen() {
    let msg = ControlMsg::SetBitrate { bps: 25_000_000 };
    let bytes = encode_bytes(&msg);
    assert_eq!(
        hex(&bytes),
        SET_BITRATE_BYTES,
        "SetBitrate 的线上字节发生漂移"
    );
    let back: ControlMsg = decode_as(&bytes).expect("应当能解回");
    assert_eq!(back, msg);
}

/// `Hello` 的线上字节被固定下来，且能原样解回。
#[test]
fn hello_bytes_are_frozen() {
    let bytes = encode_bytes(&snapshot_hello());
    assert_eq!(hex(&bytes), HELLO_BYTES, "Hello 的线上字节发生漂移");
    let back: ControlMsg = decode_as(&bytes).expect("应当能解回");
    assert_eq!(back, snapshot_hello());
}

/// `HelloAck` 的线上字节被固定下来，且能原样解回。
#[test]
fn hello_ack_bytes_are_frozen() {
    let msg = ControlMsg::HelloAck {
        agreed: Agreed {
            codec: CodecKind::H264,
            width: 1920,
            height: 1080,
            fps: 60,
            features: feature::AUDIO,
        },
    };
    let bytes = encode_bytes(&msg);
    assert_eq!(hex(&bytes), HELLO_ACK_BYTES, "HelloAck 的线上字节发生漂移");
    let back: ControlMsg = decode_as(&bytes).expect("应当能解回");
    assert_eq!(back, msg);
}

/// `PairOk` 的线上字节被固定下来，且能原样解回。
#[test]
fn pair_ok_bytes_are_frozen() {
    let msg = ControlMsg::PairOk {
        fingerprint: [0xA5; 32],
    };
    let bytes = encode_bytes(&msg);
    assert_eq!(hex(&bytes), PAIR_OK_BYTES, "PairOk 的线上字节发生漂移");
    let back: ControlMsg = decode_as(&bytes).expect("应当能解回");
    assert_eq!(back, msg);
}

/// 16 字节帧头的逐位字节被固定下来。
#[test]
fn frame_header_bytes_are_frozen() {
    let bytes = snapshot_header().encode();
    assert_eq!(bytes.len(), FRAME_HEADER_LEN);
    assert_eq!(hex(&bytes), FRAME_HEADER_BYTES, "帧头的线上字节发生漂移");

    // 逐位再核对一遍，避免十六进制串本身写错却自洽。
    assert_eq!(bytes[0], 0x10, "ver=1(type=0) 应为 0b0001_0000");
    assert_eq!(bytes[1], 0x01, "flags bit0 关键帧");
    assert_eq!(&bytes[2..6], &[0x01, 0x02, 0x03, 0x04], "frame_id 大端");
    assert_eq!(&bytes[6..8], &[0x05, 0x06], "shard_idx 大端");
    assert_eq!(bytes[8], 0x07, "data_shards");
    assert_eq!(bytes[9], 0x08, "fec_shards");
    assert_eq!(
        &bytes[10..16],
        &[0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E],
        "capture_ts 48 位大端"
    );

    let back = VideoPktHeader::decode(&bytes).expect("应当能解回");
    assert_eq!(back, snapshot_header());
}

/// 前向兼容：给旧版本消息尾部追加未知字节后，旧解码器仍能解回原消息。
///
/// 这模拟「未来版本给消息最后一个结构体追加了新字段」：
/// postcard 的结构体是位置化编码，新字段的字节正好落在消息尾部，
/// 旧解码器按 §5「postcard 末尾追加」规则忽略它。
#[test]
fn trailing_unknown_bytes_are_ignored_by_old_reader() {
    let msg = snapshot_hello();
    let mut bytes = encode_bytes(&msg);

    // 模拟新版本追加的未知字段（postcard 变长编码的若干字节）。
    bytes.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

    let back: ControlMsg = decode_as(&bytes).expect("尾部追加字节应被忽略");
    assert_eq!(back, msg, "尾部追加未知字节不得改变原有字段的解析结果");

    // 分派解码路径同样成立。
    match decode(&bytes).expect("应当能解出控制消息") {
        beam_proto::AnyMsg::Control(c) => assert_eq!(c, msg),
        other => panic!("期望控制消息,实际 {other:?}"),
    }
}

/// 前向兼容：尾部追加任意长度的未知字节都不能破坏解析。
#[test]
fn trailing_unknown_bytes_of_any_length_are_ignored() {
    let msg = snapshot_hello();
    let base = encode_bytes(&msg);
    for extra in 1..=64usize {
        let mut bytes = base.clone();
        bytes.extend(std::iter::repeat(0x5A).take(extra));
        let back: ControlMsg = decode_as(&bytes).expect("尾部追加字节应被忽略");
        assert_eq!(back, msg, "追加 {extra} 字节后解析结果发生漂移");
    }
}

/// 未来的新枚举变体不能让旧解码器误解析：必须干净地返回错误。
///
/// 控制消息变体序号按声明顺序编码，`ControlMsg` 当前最后一个变体是
/// `Error`（序号 14）。序号 15 及以后留给未来版本；
/// 旧解码器遇到它必须报错，而不是把它当成某个已知变体。
///
/// 注意：未知变体目前返回的是 `E1002`（postcard 解码失败）而不是 `E1003`。
/// `error.rs` 把 `E1003` 描述为「枚举变体序号不在已知集合内」，
/// 与此处的实际行为不一致——这是**已知的实现与文档偏差**，
/// 按 §5「协议层行为是既定契约、不擅改」的要求，这里如实固化当前行为并上报，
/// 不在本文件里改协议实现。
#[test]
fn future_enum_variant_is_rejected_not_misparsed() {
    // 族标签 01 + 变体序号 15
    let bytes = [0x01u8, 0x0F];
    let err = decode(&bytes).expect_err("未知变体必须报错");
    assert_eq!(err.code(), 1002, "未知变体当前返回 E1002");

    // 更离谱的变体序号同样只能报错。
    for variant in [0x0Fu8, 0x10, 0x7F, 0xFF] {
        let bytes = [0x01u8, variant];
        assert!(
            decode(&bytes).is_err(),
            "变体序号 {variant} 竟被解成了合法消息"
        );
    }
}
