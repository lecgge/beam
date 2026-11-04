//! # beam-net —— Beam 网络层
//!
//! 对应技术设计文档 **§3.2**（模块设计）与 **§5**（协议规格）。
//!
//! ## 职责
//!
//! quinn 端点；视频 / 音频 datagram 分包与重组；Reed-Solomon FEC；
//! 码率控制（基于丢包与 RTT 的 GCC 简化版）；接收端抖动缓冲；连接统计。
//!
//! ## 边界
//!
//! - **负责**：搬字节。分包、重组、FEC、码率决策。
//! - **不负责**：不懂编码格式；不做配对决策（TLS 校验回调由 pair 提供）。
//! - **依赖**：proto、pair（`PeerVerifier`）、quinn、reed-solomon-erasure（可选）。
//! - **被依赖**：session、ffi。
//! - **禁止**：依赖 capture / encode / decode。
//!
//! ## 接口
//!
//! - [`NetEndpoint::listen`] / [`NetEndpoint::connect`]：建端点。
//! - [`VideoSender`] / [`VideoReceiver`]：视频收发。
//! - [`VideoStream::try_next_frame`]：**非阻塞**取下一帧（轮询式取帧通道）。
//! - [`Conn::accept_uni`]：主机侧接受输入单向流（§5 输入 stream）。
//! - [`RateController::on_feedback`]：码率决策。
//!
//! ## 分帧编解码：唯一真值来源
//!
//! 控制 / 输入流的线上帧格式（**4 字节大端长度前缀 + postcard 负载 + 1 字节族标签**）
//! 由 [`framing`] 模块**统一实现**：
//!
//! - [`framing::encode_frame`]：写一帧；[`framing::FrameDecoder`]：读一帧；
//! - 该格式此前在 `beam-client` 与 `beam-host` 各写了一份（§4 不允许 L3 横向依赖），
//!   两份必须永远字节级一致；`beam-client` 已改用本模块，
//!   `beam-host` 侧那份**应删除并改用本模块**；
//! - [`framing`] 的测试把线上字节**钉成固定十六进制常量**，
//!   任何一端改动格式都会立刻暴露（往返自洽发现不了「两端同时改错」）。
//!   详见 [`framing`] 的模块文档。
//!
//! ## 数据结构
//!
//! [`EncodedFrame`] / [`AssembledFrame`] 定义在协议层（beam-proto），
//! 本 crate 通过 [`beam_proto`] 重导出，避免 §4 禁止的 L1 横向依赖。
//!
//! ## 线程模型：与 §3.2 的描述不同（登记项，不是缺失）
//!
//! §3.2 写的是「tokio 多线程运行时；发送路径专用任务，UDP GSO 批量发包；
//! 接收重组单任务；FEC 解码在 rayon 或 blocking 线程」。
//! 本 crate 的**实际实现方式**与这段描述不同，理由如下（详见 [`endpoint`] 模块文档）：
//!
//! - **发送路径**：不额外开任务。`Conn::send_datagram` 是非阻塞的，数据报进入
//!   quinn 连接自己的发送队列，由 quinn 的 per-connection driver 任务统一发包 ——
//!   「发送路径专用任务」事实上由 quinn 提供，再叠一层只会多一次队列与一次拷贝；
//! - **UDP GSO**：quinn 的 GSO 走 `quinn-udp` 的 Linux 路径（`UDP_SEGMENT`），
//!   Windows 上没有对应能力，而本项目主机侧（capture / encode）本来就是 Windows 专有
//!   ⇒ 目标平台上不适用；
//! - **接收重组**：重组器是**同步纯状态机**（无 I/O、无 async），
//!   [`VideoStream::try_next_frame`] 是**非阻塞轮询**原语，由会话层
//!   （`beam-session`）的唯一取帧循环驱动 ⇒ 语义上仍是「单任务重组」；
//! - **FEC 解码**：在接收任务内同步完成。`rayon` 不是 workspace 依赖（引入它违反
//!   「不新增依赖」）；改用 `spawn_blocking` 需要把 `Reassembler` /
//!   `try_next_frame` 改成 async，会溢出到 `beam-session` / `beam-host` 等
//!   本 crate 无权修改的调用方。
//!
//! ⇒ 这三条登记为「**实现方式与文档描述不同**」。
//! FEC 编解码的真实耗时见 `benches/packetize.rs`（§3.2 性能预算的验证基准）。
//!
//! ## 错误码
//!
//! `E2xxx`（§3.2）：`2001` 连接超时、`2002` 证书拒绝、`2003` datagram 不支持、
//! `2004` 帧过期丢弃；另有同段扩展 `2005`–`2008`。
//!
//! - `E2002`（证书拒绝）有**真实产生路径**：对端证书被本端校验器拒绝时，
//!   rustls 中止握手、quinn 以 `CRYPTO_ERROR` 结束连接，
//!   [`endpoint`] 把它翻译成 `E2002`（而不是笼统的 `E2008`）。
//!
//! ## 配置
//!
//! [`NetConfig`]：`fec_ratio`（默认 0.2，>150 Mbps 降到 0.1）、`max_datagram=1200`、
//! `jitter_frames`、`bitrate_min/max`。
//!
//! - `fec_ratio` 的**自适应规则真的进了发送路径**：[`Conn::current_fec_ratio`]
//!   按 §5 的规则（码率 >150 Mbps → 0.1；丢包 >2% → 0.3）取值，
//!   链路估计由 [`Conn::update_link_estimate`] 喂进来。
//! - `max_datagram` 会被**连接实际能承载的数据报上限夹一刀**
//!   （[`Conn::effective_max_datagram`]）：路径 MTU 掉下来时 1200 B 的数据报
//!   直接发不出去，不夹一刀整条视频链路会彻底哑掉。
//!
//! ## feature 门控：`fec`
//!
//! §3.2 的里程碑是「MVP(无 FEC)/v1(FEC+码率控制+抖动缓冲)」，
//! 所以 FEC 必须能被关掉。本 crate 提供 **`fec`** feature：
//!
//! | 构建 | 命令 | FEC 行为 |
//! |---|---|---|
//! | MVP（默认） | `cargo build -p beam-net` | **不产生冗余分片**：`m` 恒为 0、帧头 `fec_shards` 恒为 0、接收端不做 FEC 恢复 |
//! | v1 | `cargo build -p beam-net --features fec` | 按 `ratio` 生成 `m = ceil(k×ratio)` 片冗余，接收端用冗余恢复丢片 |
//!
//! - `default = []`，因此**默认构建就是 MVP 行为**，FEC 不会「恒开」。
//! - feature 关闭时 `reed-solomon-erasure` 整个依赖都不参与编译
//!   （`optional = true` + `dep:` 语法），MVP 构建里没有 RS 代码。
//! - **配置项 `fec_ratio` 保留但不生效**：字段仍然存在、仍然会被读取与存档，
//!   但关闭 feature 时 [`NetConfig::effective_fec_ratio`] 恒返回 `0.0`，
//!   发送端因此不会产生任何冗余分片 —— §5 默认 0.2 对应的那 20% 带宽在 MVP 阶段被省掉。
//! - 编译期常量 [`fec::FEC_ENABLED`] 可以让调用方知道当前构建的形态。
//! - 两端必须同时打开或同时关闭：关闭 FEC 的接收端拿到带冗余的帧也恢复不了丢片。
//!
//! ## 帧头语义：`shard_idx` 是「组内分片号」
//!
//! §5 帧头表规定 `shard_idx | 16 | 组内分片号`，本 crate 按此实现
//! （`shard_idx` 取值 `0..k+m-1`）。16 字节帧头的位宽已经占满，
//! 没有空闲字段放「组号」，因此组号放在帧头 `flags` 字节的 **bit3..bit7**
//! （§5 只命名了 bit0 关键帧 / bit1 帧末片 / bit2 切片边界）。
//! 帧头布局与各字段位宽**没有变化**，变的只是那 5 个原本闲置的位的语义。
//! 详见 [`datagram`] 模块文档「组号是怎么恢复的」。
//!
//! ## 拆组上限：200 而不是 255（§5 的自相矛盾之处，按代码现状记录）
//!
//! §5 同时写了「`k ≤ 200`」与「单帧超过 255 片时拆成多个组」两条互斥规则。
//! 本 crate 的现状是按 **200** 拆组；255 只用于限制单组总分片数 `k + m`。
//! 详见 [`datagram`] 模块文档「拆组上限：200 而不是 255」。
//!
//! ## 抖动缓冲（§3.2 `jitter_frames`）
//!
//! [`JitterBuffer`] 实现 §3.2 的「接收端抖动缓冲」：深度来自
//! [`NetConfig::jitter_frames`]（有线 0，Wi-Fi 0–1），深度 0 时不增加任何延迟。
//! 详见 [`jitter`] 模块文档。
//!
//! ## 非阻塞取帧（帧通道）
//!
//! [`VideoStream::try_next_frame`] 是轮询式取帧的原语：
//!
//! - 只消费**已经到达**的数据报，不等待网络；没有完整帧时返回 `Ok(None)`；
//! - **不完整帧会被保留**：分片先进入重组器，等后续分片补齐，
//!   `Ok(None)` 只表示「此刻还没有完整帧」，不表示数据丢了；
//! - **过期帧会被丢弃**：超过 `frame_timeout_us` 仍未补齐的帧被清理，
//!   帧号通过 [`VideoStream::take_expired`] 取出，调用方记为 **`E2004`**
//!   （`NetError::FrameExpired`）；
//! - 完整帧先过抖动缓冲再交出去。
//!
//! ## 性能
//!
//! 分包 + FEC 编码 T1 ≤0.8 ms、T2 ≤0.4 ms、T3 ≤0.3 ms；重组 + FEC 恢复同量级。
//!
//! **实测（`cargo bench -p beam-net --features fec --bench packetize`，
//! Windows 11 x64 / rustc 1.86.0 / `bench` profile）当前是超标的**，
//! 具体数字与原因分析见 `benches/packetize.rs` 的模块文档。
//!
//! ## 音频分包：尚未实现（登记项）
//!
//! §3.2 的职责写的是「视频/**音频** datagram 分包与重组」，§5 的帧头 `type`
//! 也留了「1 音频」。本 crate 目前只有**视频**路径：
//! [`packetize`] 产出的数据片 `type = 0`（视频）、冗余片 `type = 2`。
//!
//! 补齐音频路径需要一个 §5 级别的决定，因此**没有擅自实现**：
//! 冗余片的 `type` 是 `2`，它无法表达「这组冗余属于音频帧」，
//! 接收端因而无法区分「音频帧的冗余片」与「视频帧的冗余片」——
//! 除非帧号空间也分开，或者帧头再腾出一个类型位。
//! 这属于协议语义问题，留给上层裁决。
//!
//! ## 测试
//!
//! 丢包 / 乱序 / 抖动注入、FEC 恢复率、码率控制仿真。
//!
//! - **故障注入**：`tests/support/mod.rs` 是一个**确定性**的 UDP 故障注入代理
//!   （丢包 / 乱序 / 抖动 / 坏包），`tests/fault_injection.rs` 用它做端到端断言；
//! - **FEC 恢复率**：`tests/fec_recovery.rs` 按丢片数扫描恢复率，
//!   实测「丢片数 ≤ 冗余片数 ⇒ 恢复率 100%；超出 ⇒ 0% 且绝不产出坏帧」；
//! - **性能基准**：`benches/packetize.rs`（criterion）。

#![forbid(unsafe_code)]

pub mod config;
pub mod datagram;
pub mod demux;
pub mod endpoint;
pub mod error;
pub mod fec;
pub mod framing;
pub mod jitter;
pub mod rate;

pub use beam_proto::{AssembledFrame, EncodedFrame, Feedback};

/// 对端证书校验器（定义在配对层，§4 允许 net → pair）。
///
/// 这里重导出，使 client / ffi 不必直接依赖 beam-pair 也能注入校验器。
pub use beam_pair::PeerVerifier;

pub use config::{adaptive_fec_ratio, NetConfig, TlsIdentity};
pub use datagram::{
    group_index, packetize, packetize_with_codec, with_group_index, EncodedFrameView, Packet,
    Reassembler, GROUP_INDEX_MASK, GROUP_INDEX_SHIFT, MAX_GROUPS_PER_FRAME,
};
pub use demux::DemuxStream;
pub use endpoint::{Conn, NetEndpoint, VideoReceiver, VideoSender, VideoStream};
pub use error::{message, NetError};
pub use fec::{parity_shard_count, FecCodec, FecGroup, FEC_ENABLED};
pub use framing::{decode_payload, encode_frame, FrameDecoder, LENGTH_PREFIX_LEN, MAX_FRAME_LEN};
pub use jitter::JitterBuffer;
pub use rate::{BitrateDecision, RateAction, RateController};

/// ALPN 标识（§5）。
pub const ALPN: &[u8] = beam_proto::ALPN;

/// 本地单调时钟读数（微秒）。
///
/// 与主机的 `capture_ts` 时钟**无关**：帧超时判定、抖动缓冲的停留时间
/// 都用这个时钟，避免两端时钟不同步导致误判。
///
/// 起点是本进程第一次调用 [`now_us`] 的时刻，因此读数只在进程内有意义。
pub fn now_us() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_us_is_monotonic() {
        let a = now_us();
        let b = now_us();
        assert!(b >= a, "单调时钟读数不能回退");
    }
}
