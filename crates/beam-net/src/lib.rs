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
//! ## 数据结构
//!
//! [`EncodedFrame`] / [`AssembledFrame`] 定义在协议层（beam-proto），
//! 本 crate 通过 [`beam_proto`] 重导出，避免 §4 禁止的 L1 横向依赖。
//!
//! ## 线程模型
//!
//! tokio 多线程运行时；发送路径专用任务，UDP GSO 批量发包；
//! 接收重组单任务；FEC 解码在 rayon 或 blocking 线程（当前实现在接收任务内）。
//!
//! ## 错误码
//!
//! `E2xxx`（§3.2）：`2001` 连接超时、`2002` 证书拒绝、`2003` datagram 不支持、
//! `2004` 帧过期丢弃；另有同段扩展 `2005`–`2008`。
//!
//! ## 配置
//!
//! [`NetConfig`]：`fec_ratio`（默认 0.2，>150 Mbps 降到 0.1）、`max_datagram=1200`、
//! `jitter_frames`、`bitrate_min/max`。
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
//! ## 测试
//!
//! 丢包 / 乱序 / 抖动注入、FEC 恢复率、码率控制仿真。

#![forbid(unsafe_code)]

pub mod config;
pub mod datagram;
pub mod endpoint;
pub mod error;
pub mod fec;
pub mod jitter;
pub mod rate;

pub use beam_proto::{AssembledFrame, EncodedFrame, Feedback};

/// 对端证书校验器（定义在配对层，§4 允许 net → pair）。
///
/// 这里重导出，使 client / ffi 不必直接依赖 beam-pair 也能注入校验器。
pub use beam_pair::PeerVerifier;

pub use config::{adaptive_fec_ratio, NetConfig, TlsIdentity};
pub use datagram::{
    group_index, packetize, with_group_index, EncodedFrameView, Packet, Reassembler,
    GROUP_INDEX_MASK, GROUP_INDEX_SHIFT, MAX_GROUPS_PER_FRAME,
};
pub use endpoint::{Conn, NetEndpoint, VideoReceiver, VideoSender, VideoStream};
pub use error::{message, NetError};
pub use fec::{parity_shard_count, FecCodec, FecGroup, FEC_ENABLED};
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
