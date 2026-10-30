//! 测试用的**确定性** UDP 故障注入代理（N-25）。
//!
//! ## 为什么要有它
//!
//! §3.2 的测试要求写着「丢包/乱序/抖动注入（**自写 UDP 代理**）」，
//! 而现状（见 `docs/beam-gap-list.md` 的 N-25）只有「进程内少投一片 / 倒序投递」
//! 这种**不经过真实 UDP** 的模拟，既没有抖动（时延）注入，也无法覆盖
//! 「数据报在网络里被弄坏」这类情况。
//!
//! 这个模块提供一个真正的 UDP 中继：发送端把数据报发给代理，代理按
//! **可复现规则**转发给接收端。
//!
//! ## 「可复现」是怎么保证的
//!
//! 关键在于：**代理的每一个决定都只依赖「到达序号」和「规则里的种子」**，
//! 与真实时间、线程调度、操作系统缓冲区行为完全无关。
//!
//! - 丢包：`hash64(seed, seq)`（splitmix64 终 mixing）决定丢不丢 —— 与处理顺序无关；
//! - 抖动：`release_tick = seq + delay(seq)`，代理按 `release_tick` 升序转发，
//!   所以**转发顺序是 seq 的纯函数**；
//! - 乱序：固定窗口内逆序，规则本身不含随机性。
//!
//! 因此「同一串输入 → 同一串输出」，测试可以直接断言顺序与集合。
//!
//! ## 为什么放在 `tests/` 而不是 `src/`
//!
//! 生产路径不能带这些东西。`tests/support/` 是子目录，Cargo 不会把它当测试目标，
//! 只有显式 `mod support;` 的集成测试才会编译它。

// 两个集成测试二进制共用本模块，各自只用其中一部分，
// 因此这里整体关掉 dead_code 检查。
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use beam_net::{group_index, AssembledFrame, Reassembler};
use beam_proto::frame::{FrameKind, VideoPktHeader};

/// 与处理顺序无关的确定性哈希（splitmix64 终 mixing）。
///
/// 用它而不是伪随机数发生器，是因为**每个数据报的判定必须只依赖它自己的序号**：
/// 代理一次处理一个数据报，若用有状态的 RNG，第 N 个数据报的判定就会依赖
/// 前 N-1 个的判定历史，规则就不再是「可复现的纯函数」了。
pub fn hash64(seed: u64, seq: u64) -> u64 {
    let mut z = seed.wrapping_add(seq.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 丢包规则。
#[derive(Debug, Clone)]
pub enum LossRule {
    /// 显式序号表：丢掉这些「到达序号」。
    Explicit(Vec<u64>),
    /// 伪随机丢包：按 `permille / 1000` 的比例丢（种子固定 ⇒ 完全可复现）。
    PseudoRandom {
        /// 随机种子。
        seed: u64,
        /// 千分比（1000 = 全丢）。
        permille: u32,
    },
    /// 按帧头内容丢：只丢 `type` 匹配（可选再限定组号）的数据报。
    ///
    /// 用来表达「丢掉某一组 FEC 冗余片」这种**与序号无关**的语义。
    ByHeader {
        /// 帧头 `type`（§5：0 视频 / 1 音频 / 2 FEC 冗余）。
        kind: FrameKind,
        /// 只丢这个组号的数据报；`None` 表示不限组号。
        group: Option<usize>,
    },
}

impl LossRule {
    /// 判断第 `seq` 个数据报（内容为 `data`）是否要丢。
    pub fn matches(&self, seq: u64, data: &[u8]) -> bool {
        match self {
            LossRule::Explicit(list) => list.contains(&seq),
            LossRule::PseudoRandom { seed, permille } => {
                if *permille == 0 {
                    return false;
                }
                hash64(*seed, seq) % 1000 < u64::from(*permille)
            }
            LossRule::ByHeader { kind, group } => {
                let Ok(header) = VideoPktHeader::decode(data) else {
                    return false;
                };
                if header.kind != *kind {
                    return false;
                }
                match group {
                    Some(g) => group_index(header.flags) == *g,
                    None => true,
                }
            }
        }
    }
}

/// 抖动规则（决定每个数据报被推迟多少个「时隙」）。
#[derive(Debug, Clone)]
pub enum JitterRule {
    /// 显式延迟表：第 `i` 个数据报延迟 `table[i % table.len()]` 个时隙。
    Table(Vec<u64>),
    /// 伪随机延迟：种子 + 序号决定延迟时隙数（取值 `0..=max_ticks`）。
    Random {
        /// 随机种子。
        seed: u64,
        /// 最大延迟时隙数。
        max_ticks: u64,
    },
}

impl JitterRule {
    /// 第 `seq` 个数据报的延迟时隙数。
    pub fn delay_ticks(&self, seq: u64) -> u64 {
        match self {
            JitterRule::Table(table) => {
                if table.is_empty() {
                    0
                } else {
                    table[seq as usize % table.len()]
                }
            }
            JitterRule::Random { seed, max_ticks } => {
                if *max_ticks == 0 {
                    0
                } else {
                    hash64(*seed, seq) % (max_ticks + 1)
                }
            }
        }
    }
}

/// 代理的故障规则。
#[derive(Debug, Clone)]
pub enum FaultRule {
    /// 原样转发（基线）。
    Passthrough,
    /// 丢包。
    Loss(LossRule),
    /// 乱序：每 `window` 个数据报一个窗口，窗口内**逆序**转发。
    Reorder {
        /// 窗口大小（≥2）。
        window: usize,
    },
    /// 抖动：按规则给每个数据报加确定性延迟。
    Jitter {
        /// 延迟规则。
        rule: JitterRule,
        /// 一个时隙折合多少微秒（0 = 只乱序不真的等）。
        tick_us: u64,
    },
    /// 损坏：每第 `every` 个数据报被截断成 `keep_bytes` 字节。
    Corrupt {
        /// 损坏间隔（每 `every` 个坏一个）。
        every: u64,
        /// 截断后保留的字节数。
        keep_bytes: usize,
    },
}

/// 代理的运行统计（用来断言「代理确实按规则做了事」）。
#[derive(Debug, Default, Clone)]
pub struct ProxyStats {
    /// 收到的数据报数。
    pub received: u64,
    /// 转发出去的数据报数。
    pub forwarded: u64,
    /// 丢掉的数据报数。
    pub dropped: u64,
    /// 每个被转发数据报的延迟时隙数（仅抖动规则会非零）。
    pub delays: Vec<u64>,
}

/// UDP 故障注入代理。
#[derive(Debug)]
pub struct FaultProxy {
    /// 代理的监听地址：发送端把数据报发到这里。
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<ProxyStats>>,
}

impl FaultProxy {
    /// 启动代理，转发目标是 `downstream`（真实接收端）。
    pub fn spawn(downstream: SocketAddr, rule: FaultRule) -> io::Result<Self> {
        let up = UdpSocket::bind("127.0.0.1:0")?;
        let out = UdpSocket::bind("127.0.0.1:0")?;
        let addr = up.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || run(rule, up, out, downstream, stop_flag));
        Ok(Self {
            addr,
            stop,
            handle: Some(handle),
        })
    }

    /// 代理的监听地址。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 停止代理：把缓冲里的数据报按规则发完，然后返回统计。
    pub fn finish(mut self) -> ProxyStats {
        self.stop.store(true, Ordering::SeqCst);
        self.handle
            .take()
            .expect("代理线程只会被 join 一次")
            .join()
            .expect("代理线程不应 panic")
    }
}

/// 代理主循环。
fn run(
    rule: FaultRule,
    up: UdpSocket,
    out: UdpSocket,
    downstream: SocketAddr,
    stop: Arc<AtomicBool>,
) -> ProxyStats {
    // 读超时只是为了让 stop 标志能被及时看到；超时不是错误。
    let _ = up.set_read_timeout(Some(Duration::from_millis(20)));
    let mut stats = ProxyStats::default();
    let mut buf = vec![0u8; 65_536];
    let mut seq: u64 = 0;
    // 乱序模式的窗口缓冲。
    let mut reorder_buf: Vec<Vec<u8>> = Vec::new();
    // 抖动模式：释放时隙 → 待发数据报（同一时隙内按序号升序）。
    let mut jitter_buf: BTreeMap<u64, Vec<(u64, u64, Vec<u8>)>> = BTreeMap::new();

    loop {
        let (n, _from) = match up.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                // 读超时：只有在收到停止信号后才收尾。
                // 这样「停止」= **先把 socket 里剩下的数据报读完再冲刷缓冲**，
                // 不会出现「发送端刚发完、代理就退」把数据报丢掉的竞态。
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            Err(_) => break,
        };
        let data = buf[..n].to_vec();
        let cur = seq;
        seq += 1;
        stats.received += 1;

        match &rule {
            FaultRule::Passthrough => forward(&out, downstream, &data, &mut stats),
            FaultRule::Loss(lr) => {
                if lr.matches(cur, &data) {
                    stats.dropped += 1;
                } else {
                    forward(&out, downstream, &data, &mut stats);
                }
            }
            FaultRule::Reorder { window } => {
                reorder_buf.push(data);
                let w = (*window).max(2);
                if reorder_buf.len() >= w {
                    // 窗口内逆序转发。
                    while let Some(d) = reorder_buf.pop() {
                        forward(&out, downstream, &d, &mut stats);
                    }
                }
            }
            FaultRule::Jitter { rule: jr, tick_us } => {
                let ticks = jr.delay_ticks(cur);
                // 释放时隙只依赖序号 ⇒ 转发顺序是 seq 的纯函数。
                let release = cur + ticks;
                jitter_buf
                    .entry(release)
                    .or_default()
                    .push((cur, ticks, data));
                stats.delays.push(ticks);
                // 已经「到点」的时隙可以发走了。
                let ready: Vec<u64> = jitter_buf.range(..=cur).map(|(k, _)| *k).collect();
                for key in ready {
                    if let Some(batch) = jitter_buf.remove(&key) {
                        for (_, ticks, d) in batch {
                            if *tick_us > 0 {
                                std::thread::sleep(Duration::from_micros(ticks * tick_us));
                            }
                            forward(&out, downstream, &d, &mut stats);
                        }
                    }
                }
            }
            FaultRule::Corrupt { every, keep_bytes } => {
                let d = if *every > 0 && cur % *every == 0 {
                    data[..(*keep_bytes).min(data.len())].to_vec()
                } else {
                    data
                };
                forward(&out, downstream, &d, &mut stats);
            }
        }
    }

    // 收尾：把缓冲里的按规则发完（顺序仍然是确定性的）。
    match &rule {
        FaultRule::Reorder { .. } => {
            while let Some(d) = reorder_buf.pop() {
                forward(&out, downstream, &d, &mut stats);
            }
        }
        FaultRule::Jitter { tick_us, .. } => {
            let keys: Vec<u64> = jitter_buf.keys().copied().collect();
            for key in keys {
                if let Some(batch) = jitter_buf.remove(&key) {
                    for (_, ticks, d) in batch {
                        if *tick_us > 0 {
                            std::thread::sleep(Duration::from_micros(ticks * tick_us));
                        }
                        forward(&out, downstream, &d, &mut stats);
                    }
                }
            }
        }
        _ => {}
    }

    stats
}

/// 转发一个数据报。
fn forward(out: &UdpSocket, downstream: SocketAddr, data: &[u8], stats: &mut ProxyStats) {
    if out.send_to(data, downstream).is_ok() {
        stats.forwarded += 1;
    }
}

/// 接收端：绑定一个 UDP 端口，把收到的数据报喂给重组器。
#[derive(Debug)]
pub struct Receiver {
    socket: UdpSocket,
    reassembler: Reassembler,
    /// 数据报的到达顺序：`(组号, 组内分片号)`。
    order: Vec<(usize, u16)>,
    /// 已重组出的完整帧。
    frames: Vec<AssembledFrame>,
    /// 坏数据报产生的错误。
    errors: Vec<beam_net::NetError>,
}

impl Receiver {
    /// 绑定一个本地回环端口。
    pub fn bind() -> io::Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        Ok(Self {
            socket,
            reassembler: Reassembler::new(),
            order: Vec::new(),
            frames: Vec::new(),
            errors: Vec::new(),
        })
    }

    /// 接收端的地址。
    pub fn addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// 设置重组器的帧超时（微秒）。
    pub fn set_frame_timeout_us(&mut self, us: u64) {
        self.reassembler.frame_timeout_us = us;
    }

    /// 数据报的到达顺序（`(组号, 组内分片号)`）。
    pub fn order(&self) -> &[(usize, u16)] {
        &self.order
    }

    /// 已重组出的帧数。
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// 收到 `expect` 个数据报为止（或超时），期间重组出完整帧。
    ///
    /// 返回值：`(重组出的帧, 收到的数据报数)`。
    /// 收到的数据报里若有坏包，`push_datagram` 的错误会被记下来（见 [`Receiver::errors`]）。
    pub fn receive(&mut self, expect: usize, timeout: Duration) -> (Vec<AssembledFrame>, usize) {
        let deadline = std::time::Instant::now() + timeout;
        let mut buf = vec![0u8; 65_536];
        let mut got = 0usize;
        while got < expect && std::time::Instant::now() < deadline {
            match self.socket.recv_from(&mut buf) {
                Ok((n, _)) => {
                    got += 1;
                    let dg = &buf[..n];
                    if let Ok(header) = VideoPktHeader::decode(dg) {
                        self.order
                            .push((group_index(header.flags), header.shard_idx));
                    }
                    match self.reassembler.push_datagram(dg) {
                        Ok(Some(frame)) => self.frames.push(frame),
                        Ok(None) => {}
                        Err(e) => self.errors.push(e),
                    }
                }
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
        (std::mem::take(&mut self.frames), got)
    }

    /// 收到的坏数据报所产生的错误。
    pub fn errors(&self) -> &[beam_net::NetError] {
        &self.errors
    }

    /// 清理超时帧，返回被丢弃的帧号（调用方记为 `E2004`）。
    pub fn expire(&mut self) -> Vec<u32> {
        self.reassembler.expire_stale()
    }

    /// 仍在重组中的帧数。
    pub fn pending_frames(&self) -> usize {
        self.reassembler.pending_frames()
    }
}

/// 发送端：把数据报依次发给代理。
pub fn send_to_proxy(proxy: SocketAddr, datagrams: &[Vec<u8>]) -> io::Result<()> {
    let sock = UdpSocket::bind("127.0.0.1:0")?;
    for dg in datagrams {
        sock.send_to(dg, proxy)?;
    }
    Ok(())
}
