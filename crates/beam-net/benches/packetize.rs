//! §3.2 性能预算的验证基准：分包 + FEC 编码、重组 + FEC 恢复。
//!
//! ## 为什么要有这个基准
//!
//! §3.2 给出了明确的预算：**「分包+FEC 编码 T1 ≤0.8 ms、T2 ≤0.4 ms、T3 ≤0.3 ms；
//! 重组+FEC 恢复同量级」**。N-24 记录的是「全仓没有任何 `benches/` 目录或基准测试
//! 依赖，T1/T2/T3 预算无任何实测支撑」—— 这个基准就是为了把那三个数字变成
//! **可复现的实测值**。
//!
//! ## 帧长是怎么来的
//!
//! 帧长由 §8 的档位表推出来：**取该档位码率区间的上限**，按档位帧率摊到每帧。
//!
//! | 档位 | 码率上限 | 帧率 | 每帧字节 | §3.2 预算（分包+FEC 编码） |
//! |---|---|---|---|---|
//! | T0 1080p60 | 40 Mbps | 60 | 83_333 B | 未规定（只报告） |
//! | T1 4K60 | 150 Mbps | 60 | 312_500 B | ≤ 0.8 ms |
//! | T2 1440p165 | 200 Mbps | 165 | 151_515 B | ≤ 0.4 ms |
//! | T3 1080p200 | 150 Mbps | 200 | 93_750 B | ≤ 0.3 ms |
//!
//! ## 实测结果（本机：Windows 11 x64 / rustc 1.86.0 / `bench` profile）
//!
//! 命令：`cargo bench -p beam-net --features fec --bench packetize`
//!
//! | 档位 | 分包+FEC 编码 | §3.2 预算 | 重组+FEC 恢复 | 结论 |
//! |---|---|---|---|---|
//! | T0 1080p60 | 1.00 ms | 未规定 | 1.06 ms | — |
//! | T1 4K60 | **6.55 ms** | 0.8 ms | **6.82 ms** | ❌ 超标 8.2 倍 |
//! | T2 1440p165 | **5.25 ms** | 0.4 ms | **5.45 ms** | ❌ 超标 13.1 倍 |
//! | T3 1080p200 | **1.37 ms** | 0.3 ms | **1.42 ms** | ❌ 超标 4.6 倍 |
//!
//! criterion 自己的口径（区间，单位 ms）：
//!
//! | 基准 | 区间 |
//! |---|---|
//! | `net/packetize_t0` | 1.106 – 1.168 |
//! | `net/packetize_t1` | 6.740 – 6.875 |
//! | `net/packetize_t2` | 5.398 – 5.450 |
//! | `net/packetize_t3` | 1.414 – 1.428 |
//! | `net/reassemble_t0` | 27.2 – 29.3 µs |
//! | `net/reassemble_t1` | 177.0 – 186.1 µs |
//! | `net/reassemble_t2` | 54.7 – 58.5 µs |
//! | `net/reassemble_t3` | 34.0 – 37.2 µs |
//! | `net/fec_recover_t1` | 6.981 – 7.072 |
//!
//! （不带 `fec` feature 的 MVP 构建里没有 FEC 编码，T1 的分包耗时在 0.3 ms 量级。）
//!
//! ### 为什么超标的
//!
//! RS 编码的代价是 `m × k × shard_len` 次 GF(2^8) 字节乘加，
//! 而 `m = ceil(k×ratio)`、`shard_len ≈ 总字节 / (组数 × k)`，
//! 于是**总代价正比于「每组的数据片数 k」**：
//!
//! ```text
//! 代价 ≈ ratio × k × 帧字节数
//! ```
//!
//! T1 的帧是 312 500 B、拆成 2 组、每组 `k = 132` 片 ⇒
//! 代价 ≈ 0.2 × 132 × 312 500 ≈ 8.3 MB 的 GF 乘加，
//! 纯 Rust 的 `reed-solomon-erasure` 跑这个量级约 1.3 GB/s ⇒ 约 6.5 ms，与实测吻合。
//!
//! ### 可选的降本手段（**未实施**，留给上层决策）
//!
//! 1. **把 k 调小**（组数变多）：代价正比于 k，把 `k` 从 132 降到 ~16
//!    就能进预算，代价是每帧组数从 2 增到 ~20（组号 5 位、上限 32，够用），
//!    并且每组的可容忍丢片数 `m = ceil(k×ratio)` 同步变小（粒度变细）；
//! 2. **开 SIMD**：`reed-solomon-erasure` 的 `simd-accel` 走 C 的 AVX2 实现，
//!    但该 feature 在 `target_env = "msvc"` 下**不编译 SIMD 代码**
//!    （见上游 `build.rs`），Windows 上等于白开，还会多出 `cc` 构建依赖；
//! 3. **换编码**：例如 LDPC / 折叠码，代价更低但要换依赖。
//!
//! ### 结论
//!
//! §3.2 的预算在当前实现上**不成立**，本基准的作用就是把这件事变成可证伪的数字。
//! **不要为了迎合预算而改动协议实现**：预算不达标时如实记录新数字即可。
//!
//! ## 基准清单
//!
//! | 基准 | 内容 |
//! |---|---|
//! | `net/packetize_t0..t3` | 分包（`fec` feature 打开时含 FEC 编码） |
//! | `net/reassemble_t0..t3` | 重组（无丢包，纯搬运 + 拼回负载） |
//! | `net/fec_recover_t1` | 重组 + FEC 恢复（每组丢 2 片数据片） |
//!
//! 纯搬运（无丢包）的重组耗时是 T0 28 µs / T1 181 µs / T2 57 µs / T3 36 µs，
//! 也就是说**重组本身完全在预算内，超标全部来自 FEC 编解码**。
//!
//! ## 预算判定
//!
//! 基准跑完会额外打印一行 `BUDGET CHECK`，给出 T1/T2/T3 的单次耗时与
//! 相对 §3.2 预算的结论。设置环境变量 `BEAM_BENCH_STRICT=1` 时，一旦超过预算
//! 即以非零退出码结束（用于 CI 门禁；默认只报告不失败）。

use std::hint::black_box;
use std::time::{Duration, Instant};

use beam_net::datagram::packetize;
use beam_net::{EncodedFrame, Reassembler};
use bytes::Bytes;
use criterion::{criterion_group, BatchSize, Criterion};

/// §5 默认 FEC 比例。
const FEC_RATIO: f32 = 0.2;

/// §3.2 默认 `max_datagram`。
const MAX_DATAGRAM: usize = 1200;

/// 一个性能档位（§8 档位表 + §3.2 预算）。
#[derive(Debug, Clone, Copy)]
struct Tier {
    /// 档位名（T0..T3）。
    name: &'static str,
    /// 帧率。
    fps: u32,
    /// 码率上限（bit/s），取 §8 区间上限。
    max_bps: u32,
    /// §3.2 的「分包+FEC 编码」预算；`None` 表示文档未规定。
    budget: Option<Duration>,
}

impl Tier {
    /// 每帧字节数（码率上限摊到每帧）。
    fn frame_len(&self) -> usize {
        (self.max_bps as usize / 8 / self.fps as usize).max(1)
    }
}

/// §8 的四个档位。
const TIERS: [Tier; 4] = [
    Tier {
        name: "t0",
        fps: 60,
        max_bps: 40_000_000,
        budget: None,
    },
    Tier {
        name: "t1",
        fps: 60,
        max_bps: 150_000_000,
        budget: Some(Duration::from_micros(800)),
    },
    Tier {
        name: "t2",
        fps: 165,
        max_bps: 200_000_000,
        budget: Some(Duration::from_micros(400)),
    },
    Tier {
        name: "t3",
        fps: 200,
        max_bps: 150_000_000,
        budget: Some(Duration::from_micros(300)),
    },
];

/// 构造一帧可复现的负载（内容不影响耗时，只影响数据量）。
fn tier_frame(tier: &Tier) -> EncodedFrame {
    let len = tier.frame_len();
    let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    EncodedFrame {
        frame_id: 1,
        pts_us: 0,
        capture_ts_us: 0,
        is_key: true,
        slices: vec![Bytes::from(payload)],
    }
}

/// 分包（`fec` feature 打开时含 FEC 编码）。
fn bench_packetize(c: &mut Criterion) {
    for tier in TIERS {
        let frame = tier_frame(&tier);
        let name = format!("net/packetize_{}", tier.name);
        c.bench_function(&name, |b| {
            b.iter(|| {
                let out = packetize(black_box(&frame), FEC_RATIO, MAX_DATAGRAM, false)
                    .expect("应当能分包");
                black_box(out);
            });
        });
    }
}

/// 重组（无丢包）。
fn bench_reassemble(c: &mut Criterion) {
    for tier in TIERS {
        let frame = tier_frame(&tier);
        let datagrams: Vec<Vec<u8>> = packetize(&frame, FEC_RATIO, MAX_DATAGRAM, false)
            .expect("应当能分包")
            .iter()
            .map(|p| p.to_datagram())
            .collect();
        let name = format!("net/reassemble_{}", tier.name);
        c.bench_function(&name, |b| {
            b.iter_batched(
                || datagrams.clone(),
                |dgs| {
                    let mut r = Reassembler::new();
                    let mut out = None;
                    for dg in &dgs {
                        if let Some(f) = r.push_datagram(black_box(dg)).expect("重组不应报错")
                        {
                            out = Some(f);
                        }
                    }
                    black_box(out);
                },
                BatchSize::SmallInput,
            );
        });
    }
}

/// 重组 + FEC 恢复：每组丢掉 2 片数据片，逼接收端走 RS 恢复路径。
fn bench_fec_recover(c: &mut Criterion) {
    // 只对最重的一档（T1）做：恢复路径的耗时随数据量线性增长，
    // 其余档位的结论由 T1 外推即可。
    let tier = TIERS[1];
    let frame = tier_frame(&tier);
    let packets = packetize(&frame, FEC_RATIO, MAX_DATAGRAM, false).expect("应当能分包");
    // 每组丢 2 片数据片（只丢数据片，冗余片留着做恢复材料）。
    let mut per_group: std::collections::BTreeMap<usize, usize> = Default::default();
    let mut datagrams: Vec<Vec<u8>> = Vec::with_capacity(packets.len());
    for p in &packets {
        let g = beam_net::group_index(p.header.flags);
        let is_data = matches!(p.header.kind, beam_proto::FrameKind::Video);
        let seen = per_group.entry(g).or_insert(0);
        if is_data && *seen < 2 {
            *seen += 1;
            continue;
        }
        datagrams.push(p.to_datagram());
    }
    c.bench_function("net/fec_recover_t1", |b| {
        b.iter_batched(
            || datagrams.clone(),
            |dgs| {
                let mut r = Reassembler::new();
                let mut out = None;
                for dg in &dgs {
                    if let Some(f) = r.push_datagram(black_box(dg)).expect("重组不应报错") {
                        out = Some(f);
                    }
                }
                black_box(out);
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_packetize,
    bench_reassemble,
    bench_fec_recover
);

/// 单档位的一次「分包 + FEC 编码」批量计时（取多批里最快的一批）。
fn measure_packetize(tier: &Tier) -> Duration {
    /// 单批迭代次数。
    const BATCH: u32 = 20;
    /// 批次数：取最快的一批，降低调度抖动的影响。
    const ROUNDS: u32 = 10;

    let frame = tier_frame(tier);
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        let t0 = Instant::now();
        for _ in 0..BATCH {
            let out =
                packetize(black_box(&frame), FEC_RATIO, MAX_DATAGRAM, false).expect("应当能分包");
            black_box(out);
        }
        let per = t0.elapsed() / BATCH;
        if per < best {
            best = per;
        }
    }
    best
}

/// 单档位的一次「重组 + FEC 恢复」批量计时（每组丢 2 片数据片）。
fn measure_recover(tier: &Tier) -> Duration {
    /// 单批迭代次数。
    const BATCH: u32 = 20;
    /// 批次数：取最快的一批。
    const ROUNDS: u32 = 10;

    let frame = tier_frame(tier);
    let packets = packetize(&frame, FEC_RATIO, MAX_DATAGRAM, false).expect("应当能分包");
    let mut per_group: std::collections::BTreeMap<usize, usize> = Default::default();
    let mut datagrams: Vec<Vec<u8>> = Vec::with_capacity(packets.len());
    for p in &packets {
        let g = beam_net::group_index(p.header.flags);
        let is_data = matches!(p.header.kind, beam_proto::FrameKind::Video);
        let seen = per_group.entry(g).or_insert(0);
        if is_data && *seen < 2 {
            *seen += 1;
            continue;
        }
        datagrams.push(p.to_datagram());
    }

    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        let t0 = Instant::now();
        for _ in 0..BATCH {
            let mut r = Reassembler::new();
            let mut out = None;
            for dg in &datagrams {
                if let Some(f) = r.push_datagram(black_box(dg)).expect("重组不应报错") {
                    out = Some(f);
                }
            }
            black_box(out);
        }
        let per = t0.elapsed() / BATCH;
        if per < best {
            best = per;
        }
    }
    best
}

/// 预算检查：打印 T1/T2/T3 的单次耗时与相对 §3.2 预算的结论。
///
/// 这一段不依赖 criterion 的统计口径，目的只是给出一个可证伪的、
/// 可被 CI 读取的硬数字。
fn budget_check() -> bool {
    let mut ok = true;
    println!(
        "BUDGET CHECK (fec feature = {}, ratio = {FEC_RATIO}):",
        beam_net::FEC_ENABLED
    );
    for tier in TIERS {
        let enc = measure_packetize(&tier);
        let rec = measure_recover(&tier);
        match tier.budget {
            Some(budget) => {
                let enc_ok = enc <= budget;
                let rec_ok = rec <= budget;
                ok = ok && enc_ok && rec_ok;
                println!(
                    "  {}: 分包+FEC 编码 = {:>8.3} ms (预算 {:.3} ms) {} | 重组+FEC 恢复 = {:>8.3} ms {}",
                    tier.name.to_uppercase(),
                    enc.as_secs_f64() * 1e3,
                    budget.as_secs_f64() * 1e3,
                    if enc_ok { "PASS" } else { "FAIL" },
                    rec.as_secs_f64() * 1e3,
                    if rec_ok { "PASS" } else { "FAIL" },
                );
            }
            None => {
                println!(
                    "  {}: 分包+FEC 编码 = {:>8.3} ms (文档未规定预算) | 重组+FEC 恢复 = {:>8.3} ms",
                    tier.name.to_uppercase(),
                    enc.as_secs_f64() * 1e3,
                    rec.as_secs_f64() * 1e3,
                );
            }
        }
    }
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
