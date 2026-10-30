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
//! | T0 1080p60 | 0.235 ms | 未规定 | 0.241 ms | — |
//! | T1 4K60 | **1.553 ms** | 0.8 ms | **1.614 ms** | ❌ 超标 1.9 倍 |
//! | T2 1440p165 | **0.716 ms** | 0.4 ms | **0.720 ms** | ❌ 超标 1.8 倍 |
//! | T3 1080p200 | **0.276 ms** | 0.3 ms | **0.278 ms** | ✅ **达标** |
//!
//! criterion 自己的口径（区间，单位 ms）：
//!
//! | 基准 | 区间 |
//! |---|---|
//! | `net/packetize_t0` | 0.2375 – 0.2392 |
//! | `net/packetize_t1` | 1.5543 – 1.5772 |
//! | `net/packetize_t2` | 0.7240 – 0.7331 |
//! | `net/packetize_t3` | 0.2803 – 0.2825 |
//! | `net/reassemble_t0` | 20.4 – 22.2 µs |
//! | `net/reassemble_t1` | 136.9 – 147.4 µs |
//! | `net/reassemble_t2` | 46.0 – 50.8 µs |
//! | `net/reassemble_t3` | 23.9 – 24.9 µs |
//! | `net/fec_recover_t1` | 1.7391 – 1.7642 |
//!
//! ### 「矩阵跨帧复用」带来的收益（对照基准 `net/packetize_nocache_*`）
//!
//! `packetize_nocache_*` 走的是「每帧新建编解码器」的旧路径，
//! 用来量化优化前后的差距：
//!
//! | 基准 | 区间 | 对应的复用路径 | 倍数 |
//! |---|---|---|---|
//! | `net/packetize_nocache_t0` | 1.0226 – 1.0397 ms | `net/packetize_t0` 0.2375 – 0.2392 ms | 4.3× |
//! | `net/packetize_nocache_t1` | 6.6078 – 6.7848 ms | `net/packetize_t1` 1.5543 – 1.5772 ms | 4.3× |
//! | `net/packetize_nocache_t2` | 5.2818 – 5.3999 ms | `net/packetize_t2` 0.7240 – 0.7331 ms | 7.3× |
//! | `net/packetize_nocache_t3` | 1.3734 – 1.3815 ms | `net/packetize_t3` 0.2803 – 0.2825 ms | 4.9× |
//!
//! （不带 `fec` feature 的 MVP 构建里没有 FEC 编码，T1 的分包耗时在 0.3 ms 量级。）
//!
//! ### 为什么曾经超标：瓶颈在「矩阵重建」而不在「GF 乘加」
//!
//! `ReedSolomon::new(k, m)` 要在 GF(2⁸) 上求逆一个 `k×k` 矩阵（O(k³)）。
//! 分段实测（T1 档位，`k=132`、`m=27`）：
//!
//! | 阶段 | 耗时 | 占整帧比例 |
//! |---|---|---|
//! | 整帧「分包 + FEC 编码」 | 6.635 ms | 100% |
//! | **RS 矩阵构建（`ReedSolomon::new`）** | **4.894 ms** | **74%** |
//! | RS 编码（纯 GF 乘加） | 0.773 ms | 12% |
//! | 分片拷贝 | 0.006 ms | 0.1% |
//!
//! 旧实现每帧 `FecCodec::new()` ⇒ 矩阵缓存**永不跨帧存活** ⇒ 每帧都重算这 4.894 ms。
//! 现在 [`crate::datagram::packetize_with_codec`] 让发送端（[`crate::endpoint::Conn`]）
//! 与接收端（[`crate::datagram::Reassembler`]）都**复用同一个 RS 实例**：
//!
//! - 编码矩阵只建一次；
//! - `reed-solomon-erasure` 内部的**解码矩阵 LRU 缓存**也随之命中
//!   （恢复路径因此从 6.98 ms 降到 1.75 ms，−71.7%）。
//!
//! 代价 T1 的「每帧」从 6.635 ms 降到 1.553 ms（4.3×），
//! 恢复路径从 6.98 ms 降到 1.75 ms（4.0×）。
//!
//! ### 仍然超标的部分：GF 乘加本身
//!
//! 剩下的 1.553 ms 里，约 1.5 ms 是**纯 GF 乘加**：
//! RS 编码的代价是 `m × k × shard_len` 次字节乘加，
//! 而 `m = ceil(k×ratio)`、`shard_len ≈ 总字节 / (组数 × k)`，
//! 于是**总代价正比于「每组的数据片数 k」**：
//!
//! ```text
//! 代价 ≈ ratio × k × 帧字节数
//! ```
//!
//! T1 的帧是 312 500 B、拆成 2 组、每组 `k = 132` 片 ⇒
//! 代价 ≈ 0.2 × 132 × 312 500 ≈ 8.3 MB 的 GF 乘加，
//! 纯 Rust 的 `reed-solomon-erasure` 跑这个量级约 1.5 GB/s ⇒ 约 1.5 ms，与实测吻合。
//!
//! ### 可选的进一步降本手段（**未实施**，留给上层决策）
//!
//! 1. **把 k 调小**（组数变多）：代价正比于 k，把 `k` 从 132 降到 ~16
//!    就能进预算，代价是每帧组数从 2 增到 ~20（组号 5 位、上限 32，够用），
//!    并且每组的可容忍丢片数 `m = ceil(k×ratio)` 同步变小（粒度变细）；
//!    **⚠️ 这一条属于「拆组阈值」的选择**，§5 自相矛盾（待决事项 S3），
//!    未擅自实施；
//! 2. **开 SIMD**：`reed-solomon-erasure` 的 `simd-accel` 走 C 的 AVX2 实现，
//!    但该 feature 在 `target_env = "msvc"` 下**不编译 SIMD 代码**
//!    （见上游 `build.rs`），Windows 上等于白开，还会多出 `cc` 构建依赖；
//! 3. **换编码**：例如 LDPC / 折叠码，代价更低但要换依赖。
//!
//! ### 结论
//!
//! T3 档位（1080p200）现在**在预算内**（编码 0.276 ms ≤ 0.3 ms）；
//! T1 / T2 仍然超标 1.8~1.9 倍，**瓶颈是 GF 乘加本身**，
//! 已经不是可以靠「消除重复计算」解决的问题。
//! **不要为了迎合预算而改动协议实现**：预算不达标时如实记录新数字即可。
//!
//! ## 基准清单
//!
//! | 基准 | 内容 |
//! |---|---|
//! | `net/packetize_t0..t3` | 分包（**生产路径**：复用 RS 矩阵） |
//! | `net/packetize_nocache_t0..t3` | 分包（对照：每帧重建 RS 矩阵） |
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

use beam_net::datagram::packetize_with_codec;
use beam_net::{packetize, EncodedFrame, FecCodec, Reassembler};
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

/// 分包（`fec` feature 打开时含 FEC 编码）——**生产路径**。
///
/// 生产路径复用同一个 [`FecCodec`]（RS 矩阵只建一次），
/// 见 [`beam_net::endpoint::Conn::fec`]。
fn bench_packetize(c: &mut Criterion) {
    for tier in TIERS {
        let frame = tier_frame(&tier);
        let name = format!("net/packetize_{}", tier.name);
        let codec = FecCodec::new();
        c.bench_function(&name, |b| {
            b.iter(|| {
                let out =
                    packetize_with_codec(black_box(&frame), FEC_RATIO, MAX_DATAGRAM, false, &codec)
                        .expect("应当能分包");
                black_box(out);
            });
        });
    }
}

/// 分包 —— **每帧重建 RS 矩阵**的对照路径。
///
/// 用来量化「矩阵跨帧复用」到底省了多少：这条路径每次调用
/// [`packetize`]，内部新建 [`FecCodec`]，于是 `ReedSolomon::new(k, m)`
/// 的 GF(2⁸) 矩阵求逆每帧都要重做一遍。
fn bench_packetize_nocache(c: &mut Criterion) {
    for tier in TIERS {
        let frame = tier_frame(&tier);
        let name = format!("net/packetize_nocache_{}", tier.name);
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
        // 生产形态：重组器每个连接一个，但它内部的 RS 编解码器
        // 与别的重组器共享 ⇒ 解码矩阵只算一次。
        let codec = FecCodec::new();
        c.bench_function(&name, |b| {
            b.iter_batched(
                || datagrams.clone(),
                |dgs| {
                    let mut r = Reassembler::with_codec(codec.clone());
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
        // 复用同一个编解码器 ⇒ `reed-solomon-erasure` 内部的
        // 解码矩阵 LRU 缓存命中，恢复路径不再每帧重算矩阵。
        let codec = FecCodec::new();
        b.iter_batched(
            || datagrams.clone(),
            |dgs| {
                let mut r = Reassembler::with_codec(codec.clone());
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
    bench_packetize_nocache,
    bench_reassemble,
    bench_fec_recover
);

/// 单档位的一次「分包 + FEC 编码」批量计时（取多批里最快的一批）。
///
/// 走**生产路径**：复用同一个 [`FecCodec`]（RS 矩阵只建一次）。
fn measure_packetize(tier: &Tier) -> Duration {
    /// 单批迭代次数。
    const BATCH: u32 = 20;
    /// 批次数：取最快的一批，降低调度抖动的影响。
    const ROUNDS: u32 = 10;

    let frame = tier_frame(tier);
    let codec = FecCodec::new();
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        let t0 = Instant::now();
        for _ in 0..BATCH {
            let out =
                packetize_with_codec(black_box(&frame), FEC_RATIO, MAX_DATAGRAM, false, &codec)
                    .expect("应当能分包");
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
    // 共享同一个编解码器 ⇒ 解码矩阵（`reed-solomon-erasure` 内部的
    // LRU 缓存）只算一次，与生产形态一致。
    let codec = FecCodec::new();
    for _ in 0..ROUNDS {
        let t0 = Instant::now();
        for _ in 0..BATCH {
            let mut r = Reassembler::with_codec(codec.clone());
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
