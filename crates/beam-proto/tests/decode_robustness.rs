//! §3.1 测试要求：解码健壮性（退化 / 对抗输入）。
//!
//! ## 为什么不是 cargo-fuzz
//!
//! 标准 cargo-fuzz 目标需要 nightly toolchain 与 `cargo-fuzz` 工具链；
//! 本机当前只有 stable 1.86.0（`rustup toolchain list` 仅 stable）。
//! **本批以 proptest 退化测试替代 cargo-fuzz，标准 fuzz 目标待 nightly 环境就绪后补**
//! （`fuzz/` 目录已按 cargo-fuzz 标准布局就位，届时执行
//! `cargo +nightly fuzz run decode_any` 即可）。
//!
//! ## 覆盖的目标
//!
//! 与模糊测试的目的一致：**解码器面对任意畸形输入只能返回 `Err`，绝不能 panic**。
//! 覆盖的退化形态：
//!
//! - 随机字节；
//! - 截断字节（合法消息的任意前缀，含 postcard 半截字段）；
//! - 超长字节（合法消息 + 大量尾部垃圾）；
//! - 非法消息族标签；
//! - 未知枚举变体；
//! - 帧头长度不足 / 任意字节；
//! - 超大长度前缀（防止解码器按攻击者给的长度预分配巨量内存）。
//!
//! 断言用 `catch_unwind` 显式表达「不得 panic」：
//! 一旦解码器内部 panic，这里会把 panic 的输入原样打印出来，便于复现。

use std::panic::{catch_unwind, AssertUnwindSafe};

use beam_proto::caps::{feature, Caps, CodecKind, ProtocolVersion, Resolution};
use beam_proto::frame::{VideoPktHeader, FRAME_HEADER_LEN};
use beam_proto::message::{ControlMsg, Hello};
use beam_proto::{decode, decode_as, encode, AnyMsg};
use bytes::BytesMut;
use proptest::prelude::*;

/// 帧头长度（字节），与协议层常量一致。
const HEADER_LEN: usize = FRAME_HEADER_LEN;

/// 把字节渲染成十六进制串，失败时便于定位畸形输入。
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 调用 `f`；若 panic 则返回 `None`，否则返回 `Some(返回值)`。
///
/// 用 `.ok()` 而不是 `.unwrap()`，避免要求 `Box<dyn Any>` 实现 `Debug`。
fn try_call<T, F: FnOnce() -> T>(f: F) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}

/// 测试用的控制消息（`Hello`）。
fn sample_control() -> ControlMsg {
    ControlMsg::Hello(Hello {
        ver: ProtocolVersion::CURRENT,
        caps: Caps {
            codecs: vec![CodecKind::H264, CodecKind::Hevc],
            max_res: Resolution::new(1920, 1080),
            max_fps: 60,
            features: feature::AUDIO,
        },
    })
}

/// 测试用控制消息的线上字节。
fn sample_control_bytes() -> Vec<u8> {
    let mut buf = BytesMut::new();
    encode(&sample_control(), &mut buf).expect("编码应当成功");
    buf.to_vec()
}

proptest! {
    /// 随机字节：`decode` 只能返回 `Ok` 或 `Err`，绝不能 panic。
    #[test]
    fn decode_never_panics_on_random_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..=256)
    ) {
        let snapshot = hex(&bytes);
        let out = try_call(|| decode(&bytes));
        prop_assert!(out.is_some(), "decode 在随机字节上 panic 了: {snapshot}");
    }

    /// 截断字节：合法消息的任意**真前缀**都必须解码失败，且不得 panic。
    ///
    /// 前缀长度覆盖到 postcard 多字节变长字段（例如分辨率 `1920` 的
    /// `0x80 0x0f`）的中间，因此「半截字段」也被覆盖。
    #[test]
    fn decode_never_panics_on_truncated_message(cut in 0usize..=64) {
        let full = sample_control_bytes();
        // 取模映射到「真前缀」范围：0..full.len()，不含完整消息本身。
        let cut = cut % full.len();
        let slice = &full[..cut];
        let snapshot = hex(slice);
        let out = try_call(|| decode(slice));
        prop_assert!(out.is_some(), "decode 在截断输入上 panic 了: {snapshot}");
        let inner = out.expect("上面已经断言过没有 panic");
        prop_assert!(
            inner.is_err(),
            "截断的消息本该失败,却解出了 {inner:?}: {snapshot}"
        );
    }

    /// 超长字节：合法消息 + 大量尾部垃圾，必须仍能解回原消息，且不得 panic。
    #[test]
    fn decode_never_panics_on_oversized_input(
        extra in prop::collection::vec(any::<u8>(), 0..=2048)
    ) {
        let mut bytes = sample_control_bytes();
        bytes.extend_from_slice(&extra);
        let snapshot = hex(&bytes);
        let out = try_call(|| decode(&bytes));
        prop_assert!(out.is_some(), "decode 在超长输入上 panic 了: {snapshot}");
        let inner = out.expect("上面已经断言过没有 panic");
        match inner {
            Ok(AnyMsg::Control(c)) => prop_assert_eq!(c, sample_control()),
            other => prop_assert!(false, "尾部垃圾改变了解析结果: {other:?}"),
        }
    }

    /// 非法消息族标签：一律 `E1003`，且不得 panic。
    #[test]
    fn unknown_family_tag_is_rejected(tag in 0u8..=255) {
        // 1 = 控制、2 = 输入、3 = 统计，其余都是非法族标签。
        prop_assume!(!matches!(tag, 1..=3));
        let bytes = [tag, 0x00];
        let out = try_call(|| decode(&bytes));
        prop_assert!(out.is_some(), "decode 在非法族标签上 panic 了: {tag:#04x}");
        let inner = out.expect("上面已经断言过没有 panic");
        let err = inner.expect_err("非法族标签必须失败");
        prop_assert_eq!(err.code(), 1003);
    }

    /// 未知枚举变体：必须失败，且不得 panic、不得误解析成别的变体。
    #[test]
    fn unknown_enum_variant_is_rejected(variant in 15u8..=255) {
        // 族标签 01（控制）+ 控制消息变体序号。
        let bytes = [0x01u8, variant];
        let snapshot = hex(&bytes);
        let out = try_call(|| decode(&bytes));
        prop_assert!(out.is_some(), "decode 在未知变体上 panic 了: {snapshot}");
        let inner = out.expect("上面已经断言过没有 panic");
        prop_assert!(
            inner.is_err(),
            "未知变体竟被解成了 {inner:?}: {snapshot}"
        );
    }

    /// `decode_as` 拿到别的族的消息：必须失败，且不得 panic。
    #[test]
    fn decode_as_rejects_mismatched_family(
        payload in prop::collection::vec(any::<u8>(), 0..=64)
    ) {
        // 族标签 02（输入事件），却按控制消息去解。
        let mut bytes = vec![0x02u8];
        bytes.extend_from_slice(&payload);
        let snapshot = hex(&bytes);
        let out = try_call(|| decode_as::<ControlMsg>(&bytes));
        prop_assert!(out.is_some(), "decode_as 在族不匹配时 panic 了: {snapshot}");
        let inner = out.expect("上面已经断言过没有 panic");
        let err = inner.expect_err("族不匹配必须失败");
        prop_assert_eq!(err.code(), 1003);
    }

    /// 帧头解码：任意字节都不得 panic；长度不足必然失败，长度足够必然成功。
    ///
    /// 帧头是定长位解析，没有「非法取值」的概念，因此行为是确定的。
    #[test]
    fn frame_header_decode_never_panics(
        bytes in prop::collection::vec(any::<u8>(), 0..=64)
    ) {
        let snapshot = hex(&bytes);
        let out = try_call(|| VideoPktHeader::decode(&bytes));
        prop_assert!(out.is_some(), "帧头解码在任意字节上 panic 了: {snapshot}");
        let inner = out.expect("上面已经断言过没有 panic");
        if bytes.len() < HEADER_LEN {
            let err = inner.expect_err("长度不足必须失败");
            prop_assert_eq!(err.code(), 1002);
        } else {
            prop_assert!(inner.is_ok(), "长度足够时帧头必须能解出来");
        }
    }
}

/// 对抗输入：`Vec` 长度前缀声明为超大值时，解码器必须直接失败，
/// 而**不能**按攻击者给的长度去预分配内存。
///
/// 线上字节：族标签 `01` + 变体序号 `00`(=Hello) + ver.major `01` +
/// ver.minor `00` + `codecs` 的长度前缀 `ff ff ff ff 0f`（≈ 4 GiB）。
#[test]
fn huge_sequence_length_prefix_is_rejected_without_allocating() {
    let bytes = [0x01u8, 0x00, 0x01, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
    let out = try_call(|| decode(&bytes));
    assert!(out.is_some(), "解码超大长度前缀时 panic 了");
    let inner = out.expect("上面已经断言过没有 panic");
    let err = inner.expect_err("超大长度前缀必须失败");
    assert_eq!(err.code(), 1002);
}

/// 对抗输入：`String` 长度前缀声明为超大值时同理（`PairRequest{token}`）。
///
/// 线上字节：族标签 `01` + 变体序号 `02`(=PairRequest) +
/// `token` 的长度前缀 `ff ff ff ff 0f`（≈ 4 GiB）。
#[test]
fn huge_string_length_prefix_is_rejected_without_allocating() {
    let bytes = [0x01u8, 0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
    let out = try_call(|| decode(&bytes));
    assert!(out.is_some(), "解码超大长度前缀时 panic 了");
    let inner = out.expect("上面已经断言过没有 panic");
    let err = inner.expect_err("超大长度前缀必须失败");
    assert_eq!(err.code(), 1002);
}

/// 对抗输入：帧头长度不足 16 字节时必须返回 `E1002`，不得 panic。
#[test]
fn short_frame_header_is_rejected() {
    for len in 0..HEADER_LEN {
        let bytes = vec![0xFFu8; len];
        let out = try_call(|| VideoPktHeader::decode(&bytes));
        assert!(out.is_some(), "解码 {len} 字节帧头时 panic 了");
        let inner = out.expect("上面已经断言过没有 panic");
        let err = inner.expect_err("长度不足必须失败");
        assert_eq!(err.code(), 1002);
    }
}

/// 对抗输入：空消息必须失败，不得 panic。
#[test]
fn empty_input_is_rejected() {
    let out = try_call(|| decode(&[]));
    assert!(out.is_some(), "解码空输入时 panic 了");
    let inner = out.expect("上面已经断言过没有 panic");
    let err = inner.expect_err("空输入必须失败");
    assert_eq!(err.code(), 1002);
}
