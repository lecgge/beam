//! §3.1 性能预算的验证基准：单消息编解码 < 2 µs。
//!
//! 覆盖 §3.1 要求的最小集合（控制消息编解码 + 16 字节帧头编解码）：
//!
//! | 基准 | 内容 |
//! |---|---|
//! | `proto/encode_control` | 编码一条控制消息（`Hello`） |
//! | `proto/decode_control` | 解码一条控制消息（`AnyMsg` 分派路径） |
//! | `proto/decode_as_control` | 解码一条控制消息（定类型路径） |
//! | `proto/encode_frame_header` | 编码 16 字节帧头 |
//! | `proto/decode_frame_header` | 解码 16 字节帧头 |
//!
//! ## 实测结果（本机：Windows 11 x64 / rustc 1.86.0 / `bench` profile）
//!
//! 由 `cargo bench -p beam-proto` 产出（criterion 的 time 区间，单位 ns）。
//! 下表是**连续三次**运行的实测值区间；本机为共享环境，`encode` 一路
//! 在三次运行之间有 73–228 ns 的抖动，但**最差的一次也只有预算的约 11%**。
//!
//! | 基准 | 第 1 次 | 第 2 次 | 第 3 次 | §3.1 预算 |
//! |---|---|---|---|---|
//! | `proto/encode_control` | 225.0 – 227.9 | 73.3 – 74.3 | 100.4 – 118.4 | < 2000 ns ✅ |
//! | `proto/decode_control` | 58.5 – 58.9 | 59.1 – 60.1 | 75.6 – 80.8 | < 2000 ns ✅ |
//! | `proto/decode_as_control` | 58.8 – 59.4 | 58.2 – 58.8 | 68.2 – 76.2 | < 2000 ns ✅ |
//! | `proto/encode_frame_header` | 0.94 – 0.96 ns | 0.94 – 0.95 ns | 0.96 – 0.98 ns | < 2000 ns ✅ |
//! | `proto/decode_frame_header` | 1.216 – 1.231 | 1.223 – 1.245 | 1.216 – 1.245 | < 2000 ns ✅ |
//!
//! 结论：§3.1「单消息编解码 < 2 µs」在当前实现上**有约 9 倍以上余量**
//! （最慢的观测值约 228 ns，约为预算的 11%）。写在这里是为了让后续修改
//! 能立刻看到是否偏离预算。
//! **不要为了迎合预算而改动协议实现**：预算不达标时如实记录新数字即可。
//!
//! 定稿后的第 4 次复测（与上表第 2 次一致）：`encode_control` 75.4 – 78.1 ns、
//! `decode_control` 58.8 – 59.3 ns、`decode_as_control` 58.3 – 58.9 ns、
//! `encode_frame_header` 1.02 – 1.13 ns、`decode_frame_header` 1.216 – 1.226 ns。
//!
//! ## 预算判定
//!
//! 基准跑完会额外打印一行 `BUDGET CHECK`，给出单次编解码的平均耗时与
//! 相对 2 µs 预算的结论（实测 `encode=71 ns/op, decode=57 ns/op -> PASS`）。
//! 设置环境变量 `BEAM_BENCH_STRICT=1` 时，一旦超过预算即以非零退出码结束
//! （用于 CI 门禁；默认只报告不失败）。

use std::hint::black_box;
use std::time::Instant;

use beam_proto::caps::{feature, Caps, CodecKind, ProtocolVersion, Resolution};
use beam_proto::frame::VideoPktHeader;
use beam_proto::message::{ControlMsg, Hello};
use beam_proto::{decode, decode_as, encode};
use bytes::BytesMut;
use criterion::{criterion_group, Criterion};

/// §3.1 的单消息编解码预算。
const BUDGET: std::time::Duration = std::time::Duration::from_micros(2);

/// 基准用的控制消息：`Hello`（握手首条消息，负载含能力集，字段最杂）。
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

/// 基准用的帧头：各字段取互不相同的值，便于观察逐字段写入的成本。
fn sample_header() -> VideoPktHeader {
    VideoPktHeader {
        ver: VideoPktHeader::VER,
        kind: beam_proto::FrameKind::Video,
        flags: beam_proto::FrameFlags::empty().with(beam_proto::FrameFlags::KEYFRAME),
        frame_id: 0x0102_0304,
        shard_idx: 0x0506,
        data_shards: 0x07,
        fec_shards: 0x08,
        capture_ts: 0x090A_0B0C_0D0E,
    }
}

/// 编码一条控制消息。
fn bench_encode_control(c: &mut Criterion) {
    let msg = sample_control();
    let mut buf = BytesMut::with_capacity(64);
    c.bench_function("proto/encode_control", |b| {
        b.iter(|| {
            buf.clear();
            encode(black_box(&msg), black_box(&mut buf)).expect("编码应当成功");
            black_box(&buf);
        });
    });
}

/// 解码一条控制消息（`AnyMsg` 分派路径）。
fn bench_decode_control(c: &mut Criterion) {
    let msg = sample_control();
    let mut buf = BytesMut::new();
    encode(&msg, &mut buf).expect("编码应当成功");
    let bytes = buf.freeze();
    c.bench_function("proto/decode_control", |b| {
        b.iter(|| {
            let out = decode(black_box(&bytes)).expect("解码应当成功");
            black_box(out);
        });
    });
}

/// 解码一条控制消息（定类型路径）。
fn bench_decode_as_control(c: &mut Criterion) {
    let msg = sample_control();
    let mut buf = BytesMut::new();
    encode(&msg, &mut buf).expect("编码应当成功");
    let bytes = buf.freeze();
    c.bench_function("proto/decode_as_control", |b| {
        b.iter(|| {
            let out: ControlMsg = decode_as(black_box(&bytes)).expect("解码应当成功");
            black_box(out);
        });
    });
}

/// 编码 16 字节帧头。
fn bench_encode_frame_header(c: &mut Criterion) {
    let h = sample_header();
    c.bench_function("proto/encode_frame_header", |b| {
        b.iter(|| {
            let out = black_box(&h).encode();
            black_box(out);
        });
    });
}

/// 解码 16 字节帧头。
fn bench_decode_frame_header(c: &mut Criterion) {
    let raw = sample_header().encode();
    c.bench_function("proto/decode_frame_header", |b| {
        b.iter(|| {
            let out = VideoPktHeader::decode(black_box(&raw)).expect("解码应当成功");
            black_box(out);
        });
    });
}

criterion_group!(
    benches,
    bench_encode_control,
    bench_decode_control,
    bench_decode_as_control,
    bench_encode_frame_header,
    bench_decode_frame_header
);

/// 预算检查：批量计时取每次操作的平均耗时，输出相对 2 µs 预算的结论。
///
/// ## 为什么批量计时
///
/// 本机 Windows 的 `Instant` 时钟分辨率远粗于一次编解码（单次计时会读到 0 ns），
/// 因此改成「一批 N 次 → 总时长 ÷ N」来摊掉时钟分辨率误差，
/// 再对多批取最小值以降低调度抖动的影响。
///
/// 这一段不依赖 criterion 的统计口径，目的只是给出一个可证伪的、
/// 可被 CI 读取的硬数字。
fn budget_check() -> bool {
    /// 单批迭代次数。
    const BATCH: u32 = 2_000;
    /// 批次数：取最快的一批。
    const ROUNDS: u32 = 20;

    let msg = sample_control();
    let mut buf = BytesMut::with_capacity(64);
    let mut encoded = BytesMut::new();
    encode(&msg, &mut encoded).expect("编码应当成功");
    let encoded = encoded.freeze();

    let mut encode_ns = u128::MAX;
    let mut decode_ns = u128::MAX;

    for _ in 0..ROUNDS {
        let t0 = Instant::now();
        for _ in 0..BATCH {
            buf.clear();
            encode(black_box(&msg), black_box(&mut buf)).expect("编码应当成功");
            black_box(&buf);
        }
        encode_ns = encode_ns.min(t0.elapsed().as_nanos() / u128::from(BATCH));

        let t1 = Instant::now();
        for _ in 0..BATCH {
            let out = decode(black_box(&encoded)).expect("解码应当成功");
            black_box(out);
        }
        decode_ns = decode_ns.min(t1.elapsed().as_nanos() / u128::from(BATCH));
    }

    let budget_ns = BUDGET.as_nanos();
    let ok = encode_ns <= budget_ns && decode_ns <= budget_ns;
    println!(
        "BUDGET CHECK: encode={encode_ns} ns/op, decode={decode_ns} ns/op, budget={budget_ns} ns -> {}",
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

fn main() {
    benches();
    let ok = budget_check();
    // 默认只报告；CI 门禁可用 BEAM_BENCH_STRICT=1 让超标直接失败。
    if !ok && std::env::var_os("BEAM_BENCH_STRICT").is_some() {
        std::process::exit(1);
    }
}
