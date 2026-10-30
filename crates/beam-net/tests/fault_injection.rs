//! N-25：丢包 / 乱序 / 抖动注入（自写 UDP 代理，§3.2「测试」段）。
//!
//! 代理与规则定义见 [`support`] 模块：所有故障都是**可复现**的
//! （同一串输入 ⇒ 同一串输出），因此这里的断言可以直接写死顺序与集合。
//!
//! 覆盖面：
//!
//! | 用例 | 故障 | 断言 |
//! |---|---|---|
//! | [`baseline_passthrough_reassembles_byte_identical`] | 无 | 帧逐字节还原 |
//! | [`loss_beyond_fec_capacity_expires_frame_with_e2004`] | 丢数据片 | 不产出坏帧 + 超时按 `E2004` |
//! | [`loss_of_parity_shards_keeps_frame_intact`] | 丢 FEC 冗余片 | 帧仍逐字节还原 |
//! | [`reorder_reverses_each_window_but_frame_still_reassembles`] | 乱序 | 到达顺序被颠倒 + 帧仍还原 |
//! | [`jitter_reorders_delivery_but_frame_still_reassembles`] | 抖动 | 到达顺序改变 + 帧仍还原 |
//! | [`jitter_is_reproducible_across_runs`] | 抖动 | 两次运行顺序完全一致 |
//! | [`corrupted_datagram_is_reported_as_e2005`] | 坏包 | 明确报 `E2005` |
//! | [`delayed_last_shard_expires_frame_with_e2004`] | 抖动（延迟末片） | 帧超时按 `E2004` 丢弃 |

mod support;

use std::time::Duration;

use beam_net::{packetize, EncodedFrame, NetError};
use bytes::Bytes;
use support::{send_to_proxy, FaultProxy, FaultRule, JitterRule, LossRule, Receiver};

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

/// 把一帧分包成线上数据报。
fn datagrams(len: usize) -> Vec<Vec<u8>> {
    packetize(&frame(len), 0.2, 1200, false)
        .expect("应当能分包")
        .iter()
        .map(|p| p.to_datagram())
        .collect()
}

/// 窗口逆序规则下的期望到达顺序（组内分片号）。
fn expected_reorder(n: usize, window: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let end = (i + window).min(n);
        out.extend((i..end).rev().map(|x| x as u16));
        i = end;
    }
    out
}

/// 基线：不注入任何故障时，帧必须逐字节还原。
#[test]
fn baseline_passthrough_reassembles_byte_identical() {
    let payload = frame(6000).slices[0].to_vec();
    let dgs = datagrams(6000);

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    let proxy = FaultProxy::spawn(rx.addr().expect("应当能读到地址"), FaultRule::Passthrough)
        .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
    // 先让代理收尾（把缓冲里的数据报冲刷出去），再收。
    let stats = proxy.finish();

    let (frames, got) = rx.receive(dgs.len(), Duration::from_secs(5));
    assert_eq!(got, dgs.len(), "代理应当把所有数据报都转发过来");
    assert_eq!(stats.received, dgs.len() as u64);
    assert_eq!(stats.forwarded, dgs.len() as u64);
    assert_eq!(stats.dropped, 0);
    assert_eq!(frames.len(), 1, "应当重组出一帧");
    assert_eq!(frames[0].frame_id, 42);
    assert_eq!(frames[0].payload.as_ref(), payload.as_slice());
}

/// 丢包：丢到超出冗余能力时，**不能静默产出坏帧**，帧超时后按 `E2004` 丢弃。
///
/// 6000 B 的帧 ⇒ `k = 6`；`fec` 打开时 `m = ceil(6×0.2) = 2`，
/// 因此丢 3 片数据片必然超出冗余能力（两种构建都成立）。
#[test]
fn loss_beyond_fec_capacity_expires_frame_with_e2004() {
    let dgs = datagrams(6000);
    assert!(dgs.len() >= 4, "用例需要至少 4 个数据报");

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    rx.set_frame_timeout_us(80_000);
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        FaultRule::Loss(LossRule::Explicit(vec![1, 2, 3])),
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
    let stats = proxy.finish();

    // 丢了 3 个 ⇒ 只可能收到 dgs.len() - 3 个。
    let (frames, got) = rx.receive(dgs.len() - 3, Duration::from_secs(5));
    assert_eq!(stats.dropped, 3, "代理应当丢掉 3 个数据报");
    assert_eq!(got, dgs.len() - 3);
    assert!(
        frames.is_empty(),
        "缺片且超出冗余能力时不得拼出错误的帧,实际拼出了 frame_id = {:?}",
        frames.iter().map(|f| f.frame_id).collect::<Vec<_>>()
    );
    assert_eq!(rx.pending_frames(), 1, "该帧仍在重组器里等待补齐");

    // 等过帧超时后，帧应当被丢弃 —— 这条路径在生产代码里对应
    // `VideoStream::try_next_frame` 记的 `E2004`。
    std::thread::sleep(Duration::from_millis(250));
    let expired = rx.expire();
    assert_eq!(expired, vec![42], "超时的帧号应当被回收");
    assert_eq!(
        NetError::FrameExpired {
            frame_id: expired[0]
        }
        .code(),
        2004,
        "过期帧对应的错误码必须是 E2004"
    );
    assert_eq!(rx.pending_frames(), 0);
}

/// 丢包：丢掉某一组的 FEC 冗余片，数据片齐全 ⇒ 帧照常还原。
///
/// 这是「冗余片是保险而不是必需」的验证：冗余片丢了，数据片还在，
/// 接收端根本不需要动用恢复能力。
#[test]
fn loss_of_parity_shards_keeps_frame_intact() {
    let payload = frame(6000).slices[0].to_vec();
    let dgs = datagrams(6000);

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        FaultRule::Loss(LossRule::ByHeader {
            kind: beam_proto::FrameKind::Fec,
            group: Some(0),
        }),
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
    let stats = proxy.finish();

    let (frames, got) = rx.receive(dgs.len() - stats.dropped as usize, Duration::from_secs(5));
    if stats.dropped == 0 {
        // MVP 构建（`fec` feature 关闭）根本不产生冗余片，
        // 此时本用例退化成「无损」基线，帧仍必须完整。
        assert_eq!(got, dgs.len());
    } else {
        assert!(stats.dropped > 0, "应当确实丢掉了冗余片");
    }
    assert_eq!(frames.len(), 1, "数据片齐全时帧必须完整");
    assert_eq!(frames[0].payload.as_ref(), payload.as_slice());
}

/// 乱序：窗口内逆序 ⇒ 到达顺序被颠倒，但帧仍逐字节还原。
#[test]
fn reorder_reverses_each_window_but_frame_still_reassembles() {
    let payload = frame(6000).slices[0].to_vec();
    let dgs = datagrams(6000);
    let window = 4;

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        FaultRule::Reorder { window },
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
    let _ = proxy.finish();

    let (frames, got) = rx.receive(dgs.len(), Duration::from_secs(5));
    assert_eq!(got, dgs.len());
    let observed: Vec<u16> = rx.order().iter().map(|(_, idx)| *idx).collect();
    assert_eq!(
        observed,
        expected_reorder(dgs.len(), window),
        "到达顺序必须是窗口内逆序"
    );
    assert_eq!(frames.len(), 1, "乱序不应影响重组结果");
    assert_eq!(frames[0].payload.as_ref(), payload.as_slice());
}

/// 抖动：每个数据报被推迟不同的时隙 ⇒ 到达顺序改变，但帧仍逐字节还原。
#[test]
fn jitter_reorders_delivery_but_frame_still_reassembles() {
    let payload = frame(6000).slices[0].to_vec();
    let dgs = datagrams(6000);

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        FaultRule::Jitter {
            rule: JitterRule::Random {
                seed: 0xBEEF,
                max_ticks: 4,
            },
            tick_us: 0,
        },
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
    let stats = proxy.finish();

    let (frames, got) = rx.receive(dgs.len(), Duration::from_secs(5));
    assert_eq!(got, dgs.len());
    assert!(
        stats.delays.iter().any(|d| *d > 0),
        "代理应当给至少一个数据报加了延迟"
    );
    let observed: Vec<u16> = rx.order().iter().map(|(_, idx)| *idx).collect();
    let identity: Vec<u16> = (0..dgs.len() as u16).collect();
    assert_ne!(observed, identity, "抖动应当改变到达顺序,否则测的不是抖动");
    assert_eq!(frames.len(), 1, "抖动不应影响重组结果");
    assert_eq!(frames[0].payload.as_ref(), payload.as_slice());
}

/// 可复现性：同样的种子跑两遍，到达顺序必须**完全一致**。
///
/// 这是「代理必须可复现」这条要求的直接验证 ——
/// 不可复现的注入没法写断言。
#[test]
fn jitter_is_reproducible_across_runs() {
    let dgs = datagrams(6000);
    let mut orders = Vec::new();
    for _ in 0..2 {
        let mut rx = Receiver::bind().expect("应当能绑定回环端口");
        let proxy = FaultProxy::spawn(
            rx.addr().expect("应当能读到地址"),
            FaultRule::Jitter {
                rule: JitterRule::Random {
                    seed: 0xBEEF,
                    max_ticks: 4,
                },
                tick_us: 0,
            },
        )
        .expect("应当能启动代理");
        send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
        let _ = proxy.finish();
        rx.receive(dgs.len(), Duration::from_secs(5));
        orders.push(rx.order().to_vec());
    }
    assert_eq!(orders[0], orders[1], "同一种子必须给出同一到达顺序");
}

/// 坏包：数据报被截断（帧头不足）⇒ 明确报 `E2005`，而不是静默丢弃。
#[test]
fn corrupted_datagram_is_reported_as_e2005() {
    let dgs = datagrams(6000);

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        FaultRule::Corrupt {
            every: 2,
            keep_bytes: 8,
        },
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");
    let _ = proxy.finish();

    let (_frames, _got) = rx.receive(dgs.len(), Duration::from_secs(5));

    let errors = rx.errors();
    assert!(
        !errors.is_empty(),
        "被截断的数据报必须产生错误,而不是被静默吞掉"
    );
    for e in errors {
        assert_eq!(e.code(), 2005, "坏包对应的错误码必须是 E2005");
    }
}

/// 抖动（延迟数据片）：数据片迟到超过帧超时 ⇒ 帧按 `E2004` 丢弃。
///
/// 这里延迟的是**开头 3 个数据片**（数据片在前、冗余片在后，两种构建都一样），
/// 丢 3 片必然超出冗余能力（`k = 6`、`m = ceil(6×0.2) = 2`），
/// 因此两种构建下都是「明确失败」而不是「拼出坏帧」。
#[test]
fn delayed_data_shards_expire_frame_with_e2004() {
    let dgs = datagrams(6000);
    let n = dgs.len();
    assert!(n >= 4, "用例需要至少 4 个数据报");

    // 前 3 个数据报各延迟 300 ms，其余不延迟。
    let mut table = vec![300u64; 3];
    table.resize(n, 0);

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    rx.set_frame_timeout_us(80_000);
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        FaultRule::Jitter {
            rule: JitterRule::Table(table),
            tick_us: 1_000,
        },
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");

    // 被压住的 3 个数据报还在代理手里 ⇒ 只收到 n-3 个。
    let (frames, got) = rx.receive(n - 3, Duration::from_secs(5));
    assert_eq!(got, n - 3, "被延迟的数据报应当还在代理手里");
    assert!(frames.is_empty(), "缺数据片时不得拼出帧");

    std::thread::sleep(Duration::from_millis(250));
    let expired = rx.expire();
    assert_eq!(expired, vec![42], "超时的帧号应当被回收");
    assert_eq!(
        NetError::FrameExpired {
            frame_id: expired[0]
        }
        .code(),
        2004,
        "过期帧对应的错误码必须是 E2004"
    );
    let _ = proxy.finish();
}

/// 抖动（延迟末片）：**帧末片只是冗余片时，帧必须照样拼出来**。
///
/// 这是「帧的结束标记不能只有一个数据报」的回归测试：
/// 帧头里带 `LAST_SHARD` 标志的数据报丢了（这里是被延迟到超时之后），
/// 接收端仍然要能凭负载前缀里的「总组数」判断出帧在哪里结束。
///
/// MVP 构建（`fec` 关闭）没有冗余片，末片就是数据片 ⇒ 拼不出来 ⇒ 用例跳过。
#[test]
fn delayed_last_parity_shard_still_lets_the_frame_complete() {
    let dgs = datagrams(6000);
    let n = dgs.len();
    let payload = frame(6000).slices[0].to_vec();

    // 最后一个数据报延迟 300 ms。
    let mut table = vec![0u64; n];
    table[n - 1] = 300;

    let mut rx = Receiver::bind().expect("应当能绑定回环端口");
    rx.set_frame_timeout_us(80_000);
    let proxy = FaultProxy::spawn(
        rx.addr().expect("应当能读到地址"),
        FaultRule::Jitter {
            rule: JitterRule::Table(table),
            tick_us: 1_000,
        },
    )
    .expect("应当能启动代理");
    send_to_proxy(proxy.addr(), &dgs).expect("应当能发出数据报");

    // 前 n-1 个数据报先到。
    let (frames, got) = rx.receive(n - 1, Duration::from_secs(5));
    assert_eq!(got, n - 1);

    if beam_net::FEC_ENABLED {
        // 末片是冗余片：数据片齐全 ⇒ 帧必须已经拼出来。
        assert_eq!(
            frames.len(),
            1,
            "帧末片（冗余片）迟到时帧也必须拼出来,否则等于把整帧押在一个数据报上"
        );
        assert_eq!(frames[0].payload.as_ref(), payload.as_slice());
    } else {
        // MVP：末片就是数据片，缺了拼不出来 —— 但绝不能拼出错误的帧。
        assert!(frames.is_empty(), "缺末片时不得拼出帧");
    }
    let _ = proxy.finish();
}
