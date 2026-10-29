//! # beam-proto —— Beam 串流协议层
//!
//! 对应技术设计文档 **§3.1**（模块设计）与 **§5**（协议规格）。
//!
//! ## 职责
//!
//! 所有线上消息与帧头的定义、序列化（postcard）、版本协商。
//!
//! ## 边界
//!
//! - **负责**：消息类型、16 字节大端视频帧头、版本协商、协议级数据结构。
//! - **不负责**：任何 IO、网络收发、平台调用。
//! - **依赖**：serde / postcard / bytes。
//! - **被依赖**：beam-net、beam-pair、beam-discovery、beam-session、beam-ffi。
//! - **禁止**：依赖任何平台 crate 或 tokio（§3.1 明确禁止）。
//!
//! ## 接口
//!
//! - [`encode`]：把任意 [`Message`] 序列化进 [`BytesMut`]。
//! - [`decode`]：把字节流解成 [`AnyMsg`]（控制消息 / 输入事件 / 统计上报）。
//! - [`decode_as`]：解成指定消息类型。
//! - [`negotiate_version`]：§5 版本协商（主版本不同 → `E1001`）。
//! - [`negotiate`]：能力协商（§3.1 `Caps` → [`Agreed`]）。
//!
//! ## 数据结构
//!
//! [`Hello`]、[`Caps`]、[`StreamConfig`]、[`InputEvent`]、[`StatsReport`]、
//! [`EncodedFrame`]、[`AssembledFrame`]、[`DecodedFrame`]、[`VideoPktHeader`]。
//!
//! ## 线程模型
//!
//! 纯函数，无内部状态，调用方线程内执行。
//!
//! ## 错误码
//!
//! `E1xxx`（§3.1）：`1001` 版本不兼容、`1002` 解码失败、`1003` 未知消息；
//! `1004` 能力协商失败（同段内扩展，见 [`crate::error`]）。
//!
//! ## 线上格式（实现约定）
//!
//! 控制消息使用 **postcard** 序列化；每条线上消息前置 **1 字节消息族标签**
//! （`1`=控制、`2`=输入、`3`=统计），用于 [`decode`] 分派。
//! §5 要求「次版本向后兼容，未知字段忽略，postcard 末尾追加」，
//! 因此解码使用 `postcard::take_from_bytes` 语义：**尾部多出的字节被忽略**。
//!
//! ## 性能
//!
//! 单消息编解码 < 2 µs。
//!
//! ## 测试
//!
//! proptest 往返测试、帧头逐位快照测试、版本兼容测试。

#![forbid(unsafe_code)]

pub mod caps;
pub mod error;
pub mod frame;
pub mod input;
pub mod message;
pub mod stats;

pub use caps::{
    negotiate, negotiate_version, Agreed, Caps, CodecKind, ProtocolVersion, Resolution,
    StreamConfig,
};
pub use error::{message, ProtoError};
pub use frame::{
    FrameFlags, FrameKind, VideoPktHeader, CAPTURE_TS_MASK, FRAME_HEADER_LEN, MAX_PAYLOAD_LEN,
};
pub use input::{ButtonAction, InputEvent, KeyAction, MouseButton};
pub use message::{decode, decode_as, encode, AnyMsg, ControlMsg, Feedback, Message, ALPN};
pub use stats::StatsReport;

/// 协议级「已编码视频帧」（§3.2 `EncodedFrame`）。
///
/// §4 的分层规则禁止 encode → net 的 L1 横向依赖，
/// 因此该结构放在 L0 的协议层，由 beam-net / beam-encode 共同使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// 帧号，递增（§5 `frame_id`）。
    pub frame_id: u32,
    /// 呈现时间戳，微秒（§6 单调时钟）。
    pub pts_us: u64,
    /// 主机抓屏时刻，微秒（§5 `capture_ts`）。
    pub capture_ts_us: u64,
    /// 是否关键帧（IDR）。
    pub is_key: bool,
    /// 码流切片（§6：切片用 `Bytes` 共享，零拷贝）。
    pub slices: Vec<bytes::Bytes>,
}

impl EncodedFrame {
    /// 把所有切片拼成一段连续码流。
    pub fn to_payload(&self) -> bytes::Bytes {
        if self.slices.len() == 1 {
            return self.slices[0].clone();
        }
        let total: usize = self.slices.iter().map(|s| s.len()).sum();
        let mut buf = bytes::BytesMut::with_capacity(total);
        for s in &self.slices {
            buf.extend_from_slice(s);
        }
        buf.freeze()
    }
}

/// 协议级「已重组视频帧」（§3.2 `AssembledFrame`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledFrame {
    /// 帧号，递增。
    pub frame_id: u32,
    /// 呈现时间戳，微秒。
    pub pts_us: u64,
    /// 主机抓屏时刻，微秒。
    pub capture_ts_us: u64,
    /// 是否关键帧。
    pub is_key: bool,
    /// 重组后的完整码流。
    pub payload: bytes::Bytes,
}

/// 平台图形对象的不透明句柄（§3.10 解码 → 上屏）。
///
/// §4 规定 L1 之间只有 net → pair 可以横向依赖，
/// 因此 `beam-decode` 与 `beam-render` 共享的 `DecodedFrame` 必须放在 L0，
/// 且不能携带具体平台的 COM 类型。此处用整型句柄承载平台指针，
/// 真实指针由平台 crate 自己保存与解释。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PlatformHandle {
    /// 平台指针的整数值；仅平台 crate 内部解释其含义。
    pub raw: usize,
}

impl PlatformHandle {
    /// 空句柄。
    pub const NONE: Self = Self { raw: 0 };

    /// 是否为空句柄。
    pub const fn is_none(&self) -> bool {
        self.raw == 0
    }
}

/// 解码后的帧（§3.10 `DecodedFrame`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// 帧号。
    pub frame_id: u32,
    /// 呈现时间戳，微秒。
    pub pts_us: u64,
    /// 主机抓屏时刻，微秒（用于端到端延迟统计）。
    pub capture_ts_us: u64,
    /// 解码后画面宽。
    pub width: u32,
    /// 解码后画面高。
    pub height: u32,
    /// 是否关键帧。
    pub is_key: bool,
    /// 渲染目标句柄（Windows 客户端上是 D3D11 纹理）。
    pub surface: PlatformHandle,
}
