//! N-26 / N-14：FEC 恢复率与 FEC 正确性。
//!
//! ## 实测恢复率（`--features fec`，ratio = 0.2）
//!
//! 用例 [`recovery_rate_scan_over_loss_counts`] 对每个丢片数跑 100 次确定性试验，
//! 统计「帧被完整、逐字节还原」的比例。结论写在该测试的注释与输出里：
//!
//! - 丢片数 ≤ 冗余片数 `m` ⇒ **恢复率 100%**（RS 擦除码的质）；
//! - 丢片数 > `m` ⇒ **恢复率 0%**，而且**一次都没有**产出过错误的帧
//!   （重组器保持未完成 ⇒ 最终按 `E2004` 丢弃）。
//!
//! 换句话说：ratio = 0.2 意味着**组内最多丢 20% 的分片仍然 100% 恢复**，
//! 再多就一定失败 —— 但失败是**明确的**（`E2006` / `E2004`），不是静默的坏帧。
//!
//! ## 为什么丢片是「确定性地」选的
//!
//! 每个分片是否被丢，只由 `(种子, 试验序号, 分片号)` 决定，
//! 所以同一串输入永远给出同一串输出，恢复率数字可复现、可证伪。

#![cfg(feature = "fec")]

mod support;

use std::time::Duration;

use beam_net::datagram::packetize;
use beam_net::{EncodedFrame, FecCodec, NetError, Reassembler};
use bytes::Bytes;
use support::{send_to_proxy, FaultProxy, FaultRule, LossRule, Receiver};

/// 试验次数。
const TRIALS: u64 = 100;

/// 试验种子。
const SEED: u64 = 0x5EED_1234;

/// 构造一帧可复现的负载。
fn frame(len: usize) -> EncodedFrame {
    EncodedFrame {
        frame_id: 42,
        pts_us: 1_000,
        capture_ts_us: 2_000,
        is_key: true,
        slices: vec![Bytes::from(
            (0..len).map(|i| (i % 251) as u8).collect::<Vec<u8>>(),
        )],
    }
}

/// 确定性地选出 `d` 个分片号（`0..n`）。
///
/// 用「部分 Fisher-Yates」：每一步用 `hash64` 抽一个下标并 `swap_remove`，
/// 因此选出的集合与输入顺序无关，只由 `(seed, trial)` 决定。
fn pick(n: usize, d: usize, seed: u64, trial: u64) -> Vec<usize> {
    let mut pool: Vec<usize> = (0..n).collect();
    let mut picked = Vec::with_capacity(d);
    for i in 0..d {
        let h = support::hash64(seed ^ trial.wrapping_mul(0x9E37_79B9_7F4A_7C15), i as u64);
        let idx = (h as usize) % (n - i);
        picked.push(pool.swap_remove(idx));
    }
    picked
}

/// 恢复率扫描：逐个丢片数统计恢复率，并断言「绝不产出坏帧」。
///
/// 帧长 20_000 B ⇒ `k = 17`、`m = ceil(17×0.2) = 4`，单组 21 片。
#[test]
fn recovery_rate_scan_over_loss_counts() {
    let payload = frame(20_000).slices[0].to_vec();
    let packets = packetize(&frame(20_000), 0.2, 1200, false).expect("应当能分包");
    let k = packets[0].header.data_shards as usize;
    let m = packets[0].header.fec_shards as usize;
    assert_eq!((k, m), (17, 4), "用例的分组参数必须钉死");
    assert_eq!(packets.len(), k + m);

    println!("== FEC 恢复率（k={k}, m={m}, 每次试验 {TRIALS} 次）==");
    for d in 0..=m + 1 {
        let mut recovered = 0u64;
        let mut wrong = 0u64;
        for trial in 0..TRIALS {
            let dropped = pick(k + m, d, SEED, trial);
            let mut r = Reassembler::new();
            let mut got = None;
            for p in &packets {
                if dropped.contains(&(p.header.shard_idx as usize)) {
                    continue;
                }
                if let Some(f) = r.push(&p.header, &p.payload).expect("重组不应报错") {
                    got = Some(f);
                }
            }
            match got {
                Some(f) if f.payload.as_ref() == payload.as_slice() => recovered += 1,
                Some(_) => wrong += 1,
                None => {}
            }
        }
        let rate = recovered as f64 / TRIALS as f64;
        println!(
            "  丢 {d} 片 → 恢复率 {:.1}%（{recovered} / {TRIALS}）",
            rate * 100.0
        );
        assert_eq!(wrong, 0, "恢复失败时绝不能拼出错误的帧");
        if d <= m {
            assert_eq!(recovered, TRIALS, "冗余能力之内必须 100% 恢复");
        } else {
            assert_eq!(recovered, 0, "超出冗余能力时必须明确失败");
        }
    }
}

/// 丢冗余片（只丢 parity）⇒ 帧照常还原，且**用不上**恢复能力。
#[test]
fn dropping_parity_shards_only_keeps_frame_intact() {
    let payload = frame(20_000).slices[0].to_vec();
    let packets = packetize(&frame(20_000), 0.2, 1200, false).expect("应当能分包");
    let k = packets[0].header.data_shards as usize;
    let m = packets[0].header.fec_shards as usize;

    let mut r = Reassembler::new();
    let mut got = None;
    let mut dropped = 0;
    for p in &packets {
        if p.header.shard_idx as usize >= k {
            // parity 片全丢。
            dropped += 1;
            continue;
        }
        if let Some(f) = r.push(&p.header, &p.payload).expect("重组不应报错") {
            got = Some(f);
        }
    }
    assert_eq!(dropped, m);
    assert_eq!(
        got.expect("数据片齐全时必须还原出帧").payload.as_ref(),
        payload.as_slice()
    );
}

/// 丢 1 片数据 + 1 片冗余 ⇒ 冗余能力之内，FEC 必须恢复且逐字节一致。
#[test]
fn dropping_one_data_and_one_parity_shard_recovers_byte_identical() {
    let payload = frame(20_000).slices[0].to_vec();
    let packets = packetize(&frame(20_000), 0.2, 1200, false).expect("应当能分包");

    let mut r = Reassembler::new();
    let mut got = None;
    let mut dropped = 0;
    for p in &packets {
        // 丢第 3 片（数据片）与最后一片（冗余片）。
        if p.header.shard_idx == 3 || p.header.shard_idx == 20 {
            dropped += 1;
            continue;
        }
        if let Some(f) = r.push(&p.header, &p.payload).expect("重组不应报错") {
            got = Some(f);
        }
    }
    assert_eq!(dropped, 2);
    assert_eq!(
        got.expect("FEC 应当恢复出完整帧").payload.as_ref(),
        payload.as_slice(),
        "恢复出来的帧必须与原始负载逐字节一致"
    );
}

/// 超出冗余能力 ⇒ 明确失败：编解码器报 `E2006`，帧保持未完成并最终按 `E2004` 丢弃。
#[test]
fn dropping_more_data_shards_than_parity_fails_explicitly() {
    let packets = packetize(&frame(20_000), 0.2, 1200, false).expect("应当能分包");
    let k = packets[0].header.data_shards as usize;
    let m = packets[0].header.fec_shards as usize;

    let mut r = Reassembler::new();
    let mut got = None;
    let mut dropped = 0;
    for p in &packets {
        if (p.header.shard_idx as usize) < k && dropped < m + 1 {
            dropped += 1;
            continue;
        }
        if let Some(f) = r.push(&p.header, &p.payload).expect("重组不应报错") {
            got = Some(f);
        }
    }
    assert_eq!(dropped, m + 1);
    assert!(
        got.is_none(),
        "超出冗余能力时不得拼出帧,实际拼出了 frame_id = {:?}",
        got.map(|f| f.frame_id)
    );
    assert_eq!(r.pending_frames(), 1, "帧仍在重组器里等待补齐");

    // 帧超时 ⇒ 按 E2004 丢弃（生产路径是 `VideoStream::try_next_frame`）。
    let expired = r.expire(u64::MAX / 2);
    assert_eq!(expired, vec![42]);
    assert_eq!(
        NetError::FrameExpired {
            frame_id: expired[0]
        }
        .code(),
        2004,
        "过期帧对应的错误码必须是 E2004"
    );
    assert_eq!(r.pending_frames(), 0);

    // 编解码器层面：冗余不足时必须报 E2006（FEC 不可恢复）。
    // 这里直接用公开 API 造一个「只收到 k-1 片」的组：可用片数 < k ⇒ 不可恢复。
    let data: Vec<Vec<u8>> = (0..k).map(|i| vec![i as u8; 16]).collect();
    let mut codec = FecCodec::new();
    let mut group = beam_net::FecGroup::new(k, m, 16);
    for (i, d) in data.iter().enumerate().take(k - 1) {
        assert!(group.insert(i, d));
    }
    let err = codec
        .reconstruct_group(&mut group)
        .expect_err("冗余不足时必须报 FEC 不可恢复");
    assert_eq!(err.code(), 2006, "冗余不足的错误码必须是 E2006");
}

/// 恢复能力与到达顺序无关：丢片之后再打乱顺序，仍必须恢复出逐字节一致的帧。
#[test]
fn fec_recovery_is_independent_of_arrival_order() {
    let payload = frame(20_000).slices[0].to_vec();
    let packets = packetize(&frame(20_000), 0.2, 1200, false).expect("应当能分包");

    // 丢第 2、5、9 片数据片（冗余能力之内），其余顺序完全颠倒。
    let mut kept: Vec<_> = packets
        .iter()
        .filter(|p| ![2u16, 5, 9].contains(&p.header.shard_idx))
        .cloned()
        .collect();
    kept.reverse();

    let mut r = Reassembler::new();
    let mut got = None;
    for p in &kept {
        if let Some(f) = r.push(&p.header, &p.payload).expect("重组不应报错") {
            got = Some(f);
        }
    }
    assert_eq!(
        got.expect("乱序下 FEC 仍应恢复").payload.as_ref(),
        payload.as_slice()
    );
}

/// 走真实 UDP 代理：代理丢掉 2 片数据片，FEC 必须恢复出完整帧。
#[test]
fn fec_recovers_through_the_udp_proxy() {
    let payload = frame(20_000).slices[0].to_vec();
    let dgs: Vec<Vec<u8>> = packetize(&frame(20_000), 0.2, 1200, false)
        .expect("应当能分包")
        .iter()
        .map(|p| p.to_datagram())
        .collect();

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        // 第 3、7 个数据报（到达序号）被丢掉。
        FaultRule::Loss(LossRule::Explicit(vec![3, 7])),
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
    let stats = proxy.finish();

    let (frames, got) = rx.receive(dgs.len() - 2, Duration::from_secs(5));
    assert_eq!(stats.dropped, 2);
    assert_eq!(got, dgs.len() - 2);
    assert_eq!(frames.len(), 1, "FEC 应当把丢的两片补回来");
    assert_eq!(frames[0].payload.as_ref(), payload.as_slice());
}

/// 多组帧：每组各丢 1 片数据片 ⇒ 每组各自用 FEC 恢复，整帧逐字节一致。
#[test]
fn multi_group_frame_recovers_after_loss_in_every_group() {
    let payload = frame(700_000).slices[0].to_vec();
    let packets = packetize(&frame(700_000), 0.2, 1200, false).expect("应当能分包");
    let groups: std::collections::BTreeSet<usize> = packets
        .iter()
        .map(|p| beam_net::group_index(p.header.flags))
        .collect();
    assert!(groups.len() >= 3, "用例应当覆盖 3 个以上的组");

    let mut r = Reassembler::new();
    let mut got = None;
    let mut dropped_per_group: std::collections::BTreeMap<usize, usize> = Default::default();
    for p in &packets {
        let g = beam_net::group_index(p.header.flags);
        let is_data = matches!(p.header.kind, beam_proto::FrameKind::Video);
        let dropped = dropped_per_group.entry(g).or_insert(0);
        if is_data && *dropped == 0 {
            *dropped += 1;
            continue;
        }
        if let Some(f) = r.push(&p.header, &p.payload).expect("重组不应报错") {
            got = Some(f);
        }
    }
    assert!(dropped_per_group.values().all(|n| *n == 1));
    assert_eq!(
        got.expect("多组帧应当被恢复出来").payload.as_ref(),
        payload.as_slice()
    );
}
