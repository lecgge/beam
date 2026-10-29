//! §3.1 测试要求：proptest 往返。
//!
//! 覆盖 §5 的 16 字节大端帧头逐位往返，以及控制消息的 postcard 往返。

use beam_proto::{
    caps::{feature, Caps, CodecKind, ProtocolVersion, Resolution},
    decode, decode_as, encode,
    frame::{FrameFlags, FrameKind, VideoPktHeader, CAPTURE_TS_MASK, FRAME_HEADER_LEN},
    message::{ControlMsg, Hello},
    AnyMsg,
};
use bytes::BytesMut;
use proptest::prelude::*;

proptest! {
    /// 帧头逐位往返：任取合法取值，编码后解码必须完全一致。
    #[test]
    fn frame_header_round_trip(
        ver in 0u8..=0x0F,
        kind_bits in 0u8..=0x0F,
        flags in 0u8..=0xFF,
        frame_id in any::<u32>(),
        shard_idx in any::<u16>(),
        data_shards in 1u8..=255,
        fec_shards in 0u8..=255,
        capture_ts in any::<u64>(),
    ) {
        let h = VideoPktHeader {
            ver,
            kind: FrameKind::from_bits(kind_bits),
            flags: FrameFlags::empty().with(flags),
            frame_id,
            shard_idx,
            data_shards,
            fec_shards,
            capture_ts: capture_ts & CAPTURE_TS_MASK,
        };
        let bytes = h.encode();
        prop_assert_eq!(bytes.len(), FRAME_HEADER_LEN);
        let back = VideoPktHeader::decode(&bytes).expect("帧头应当可解");
        prop_assert_eq!(back, h);
    }

    /// 控制消息 postcard 往返。
    #[test]
    fn control_msg_round_trip(
        major in 1u8..=3,
        minor in 0u8..=15,
        width in 1u16..=4096,
        height in 1u16..=4096,
        fps in 1u32..=360,
        features in any::<u64>(),
        codec_bits in 0u8..=2,
    ) {
        let msg = ControlMsg::Hello(Hello {
            ver: ProtocolVersion { major, minor },
            caps: Caps {
                codecs: vec![CodecKind::from_bits(codec_bits)],
                max_res: Resolution::new(width, height),
                max_fps: fps,
                features,
            },
        });
        let mut buf = BytesMut::new();
        encode(&msg, &mut buf).expect("编码应当成功");
        let back: ControlMsg = decode_as(&buf).expect("解码应当成功");
        prop_assert_eq!(&back, &msg);

        match decode(&buf).expect("分派解码应当成功") {
            AnyMsg::Control(c) => prop_assert_eq!(c, msg),
            _ => prop_assert!(false, "应当解出控制消息"),
        }
    }
}

/// postcard 枚举变体顺序是线上格式的一部分，固定下来防止无意改动（§5）。
#[test]
fn codec_variant_order_is_stable() {
    assert_eq!(CodecKind::H264 as u8, 0);
    assert_eq!(CodecKind::Hevc as u8, 1);
    assert_eq!(CodecKind::Av1 as u8, 2);
    assert!(Caps::default_host().has_feature(feature::AUDIO));
}
