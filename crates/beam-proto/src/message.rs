//! 控制消息、线上编解码与反馈结构（§5 控制消息清单）。
//!
//! 控制通道是 QUIC 双向可靠 stream 0，ALPN 为 `beam/1`。
//! 消息清单（§5）：Hello/HelloAck、PairRequest/PairChallenge/PairOk/PairFail、
//! StreamConfig、RequestIdr、SetBitrate、Feedback、ClockSync(t0..t3)、
//! StatsReport、Resume、Bye、Error{code}。

use bytes::{BufMut, BytesMut};
use serde::{Deserialize, Serialize};

use crate::caps::{Agreed, Caps, ProtocolVersion, StreamConfig};
use crate::error::ProtoError;
use crate::input::InputEvent;
use crate::stats::StatsReport;

/// ALPN 标识（§5）。
pub const ALPN: &[u8] = b"beam/1";

/// 消息族标签：1 字节前缀，用于 [`decode`] 分派。
pub mod family {
    /// 控制消息。
    pub const CONTROL: u8 = 1;
    /// 输入事件。
    pub const INPUT: u8 = 2;
    /// 统计上报。
    pub const STATS: u8 = 3;
}

/// 可线上编码的消息。
pub trait Message: Serialize + serde::de::DeserializeOwned {
    /// 消息族标签（1 字节前缀）。
    const FAMILY: u8;

    /// 消息种类名（仅用于日志与调试，不参与线上编码）。
    fn kind(&self) -> &'static str;
}

/// 握手消息（§5 `Hello`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// 协议版本（主版本不同直接拒绝 `E1001`）。
    pub ver: ProtocolVersion,
    /// 能力集。
    pub caps: Caps,
}

/// 网络反馈（§3.2 `Feedback{loss,rtt,recv_rate}`、§5 控制消息 `Feedback`）。
///
/// §4 禁止 net → session 之外的 L1 横向依赖，而 §5 又把 `Feedback` 列为控制消息，
/// 因此该结构放在 L0 协议层，由 beam-net 与 beam-session 共用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feedback {
    /// 丢包率，万分比（0..=10000；§5 阈值 2% = 200）。
    pub loss: u32,
    /// 往返时延。
    pub rtt: std::time::Duration,
    /// 接收码率，bit/s。
    pub recv_rate: u32,
}

impl Feedback {
    /// 丢包率是否超过 §5 的 2% 阈值。
    pub const fn loss_above_2pct(&self) -> bool {
        self.loss > 200
    }
}

/// 控制通道消息（§5 控制消息清单）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMsg {
    /// 握手：客户端 → 主机。
    Hello(Hello),
    /// 握手应答：主机 → 客户端，携带协商结果。
    HelloAck {
        /// 协商后的能力。
        agreed: Agreed,
    },
    /// 配对请求（二维码 token 方式，§3.3）。
    PairRequest {
        /// 一次性 token（§3.3 `pin_ttl=120s`）。
        token: String,
    },
    /// SPAKE2 配对挑战（§3.3）。
    PairChallenge {
        /// SPAKE2 首条消息。
        msg: Vec<u8>,
    },
    /// 配对成功，双方固定对方指纹。
    PairOk {
        /// 对端证书指纹（SHA-256，32 字节）。
        fingerprint: [u8; 32],
    },
    /// 配对失败。
    PairFail {
        /// 错误码（`E3xxx`）。
        code: u16,
        /// 失败原因（仅用于日志）。
        reason: String,
    },
    /// 串流配置变更。
    StreamConfig(StreamConfig),
    /// 请求 IDR（客户端 → 主机）。
    RequestIdr,
    /// 码率调整（客户端 → 主机）。
    SetBitrate {
        /// 目标码率，bit/s。
        bps: u32,
    },
    /// 网络反馈（客户端 → 主机）。
    Feedback(Feedback),
    /// NTP 式四时间戳时钟同步（§3.13 E2E 测量，每 5 s 一次）。
    ClockSync {
        /// 客户端发送时刻。
        t0: u64,
        /// 主机接收时刻。
        t1: u64,
        /// 主机回复时刻。
        t2: u64,
        /// 客户端接收时刻。
        t3: u64,
    },
    /// 统计上报。
    StatsReport(StatsReport),
    /// 断线重连续传（§5 `Resume{session_id}`）。
    Resume {
        /// 要恢复的会话号。
        session_id: u64,
    },
    /// 正常结束。
    Bye,
    /// 错误通知（§5 `Error{code}`）。
    Error {
        /// 错误码（`E` 段编号）。
        code: u16,
    },
}

impl ControlMsg {
    /// 消息种类名（用于日志）。
    pub const fn kind(&self) -> &'static str {
        match self {
            ControlMsg::Hello(_) => "hello",
            ControlMsg::HelloAck { .. } => "hello_ack",
            ControlMsg::PairRequest { .. } => "pair_request",
            ControlMsg::PairChallenge { .. } => "pair_challenge",
            ControlMsg::PairOk { .. } => "pair_ok",
            ControlMsg::PairFail { .. } => "pair_fail",
            ControlMsg::StreamConfig(_) => "stream_config",
            ControlMsg::RequestIdr => "request_idr",
            ControlMsg::SetBitrate { .. } => "set_bitrate",
            ControlMsg::Feedback(_) => "feedback",
            ControlMsg::ClockSync { .. } => "clock_sync",
            ControlMsg::StatsReport(_) => "stats_report",
            ControlMsg::Resume { .. } => "resume",
            ControlMsg::Bye => "bye",
            ControlMsg::Error { .. } => "error",
        }
    }
}

impl Message for ControlMsg {
    const FAMILY: u8 = family::CONTROL;

    fn kind(&self) -> &'static str {
        ControlMsg::kind(self)
    }
}

impl Message for InputEvent {
    const FAMILY: u8 = family::INPUT;

    fn kind(&self) -> &'static str {
        match self {
            InputEvent::MouseMove { .. } => "mouse_move",
            InputEvent::MouseButton { .. } => "mouse_button",
            InputEvent::Key { .. } => "key",
            InputEvent::Scroll { .. } => "scroll",
            InputEvent::GamepadAxis { .. } => "gamepad_axis",
            InputEvent::GamepadButton { .. } => "gamepad_button",
        }
    }
}

impl Message for StatsReport {
    const FAMILY: u8 = family::STATS;

    fn kind(&self) -> &'static str {
        "stats_report"
    }
}

/// 任意线上消息（§3.1 `AnyMsg`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnyMsg {
    /// 控制消息。
    Control(ControlMsg),
    /// 输入事件。
    Input(InputEvent),
    /// 统计上报。
    Stats(StatsReport),
}

/// 把消息序列化进 `buf`（前置 1 字节消息族标签）。
///
/// 预算：单消息编解码 < 2 µs（§3.1 性能）。
pub fn encode<M: Message>(m: &M, buf: &mut BytesMut) -> Result<(), ProtoError> {
    let payload = postcard::to_allocvec(m).map_err(|e| ProtoError::DecodeFailed {
        reason: format!("postcard 编码失败: {e}"),
    })?;
    buf.reserve(1 + payload.len());
    buf.put_u8(M::FAMILY);
    buf.extend_from_slice(&payload);
    Ok(())
}

/// 解出指定类型的消息。
///
/// 尾部多余字节被忽略（§5：未知字段忽略，postcard 末尾追加）。
pub fn decode_as<M: Message>(buf: &[u8]) -> Result<M, ProtoError> {
    let (tag, rest) = split_tag(buf)?;
    if *tag != M::FAMILY {
        return Err(ProtoError::UnknownMessage {
            variant: u32::from(*tag),
        });
    }
    decode_payload(rest)
}

/// 解出任意线上消息（§3.1 `decode`）。
pub fn decode(buf: &[u8]) -> Result<AnyMsg, ProtoError> {
    let (tag, rest) = split_tag(buf)?;
    match *tag {
        family::CONTROL => decode_payload::<ControlMsg>(rest).map(AnyMsg::Control),
        family::INPUT => decode_payload::<InputEvent>(rest).map(AnyMsg::Input),
        family::STATS => decode_payload::<StatsReport>(rest).map(AnyMsg::Stats),
        other => Err(ProtoError::UnknownMessage {
            variant: u32::from(other),
        }),
    }
}

fn split_tag(buf: &[u8]) -> Result<(&u8, &[u8]), ProtoError> {
    buf.split_first().ok_or_else(|| ProtoError::DecodeFailed {
        reason: "空消息".to_string(),
    })
}

fn decode_payload<M: Message>(buf: &[u8]) -> Result<M, ProtoError> {
    let (m, _trailing) =
        postcard::take_from_bytes::<M>(buf).map_err(|e| ProtoError::DecodeFailed {
            reason: format!("postcard 解码失败: {e}"),
        })?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{feature, CodecKind, Resolution};

    fn sample_control() -> ControlMsg {
        ControlMsg::Hello(Hello {
            ver: ProtocolVersion::CURRENT,
            caps: Caps {
                codecs: vec![CodecKind::Hevc],
                max_res: Resolution::new(1920, 1080),
                max_fps: 60,
                features: feature::AUDIO,
            },
        })
    }

    #[test]
    fn control_round_trip() {
        let msg = sample_control();
        let mut buf = BytesMut::new();
        encode(&msg, &mut buf).expect("编码应当成功");
        let back: ControlMsg = decode_as(&buf).expect("解码应当成功");
        assert_eq!(msg, back);
    }

    #[test]
    fn decode_dispatches_by_family_tag() {
        let msg = sample_control();
        let mut buf = BytesMut::new();
        encode(&msg, &mut buf).expect("编码应当成功");
        match decode(&buf).expect("解码应当成功") {
            AnyMsg::Control(c) => assert_eq!(c, msg),
            other => panic!("期望控制消息,实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_family_tag_is_e1003() {
        let err = decode(&[0x7f, 0x00]).expect_err("未知消息族必须失败");
        assert_eq!(err.code(), 1003);
    }

    #[test]
    fn trailing_bytes_are_ignored_for_forward_compatibility() {
        let msg = sample_control();
        let mut buf = BytesMut::new();
        encode(&msg, &mut buf).expect("编码应当成功");
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        let back: ControlMsg = decode_as(&buf).expect("尾部追加字节应被忽略");
        assert_eq!(msg, back);
    }

    #[test]
    fn broken_payload_is_e1002() {
        let err = decode(&[family::CONTROL, 0xFF, 0xFF]).expect_err("非法负载必须失败");
        assert_eq!(err.code(), 1002);
    }

    #[test]
    fn every_control_variant_round_trips() {
        let msgs = vec![
            ControlMsg::HelloAck {
                agreed: Agreed {
                    codec: CodecKind::H264,
                    width: 1920,
                    height: 1080,
                    fps: 60,
                    features: 0,
                },
            },
            ControlMsg::PairRequest {
                token: "tok".into(),
            },
            ControlMsg::PairChallenge { msg: vec![1, 2, 3] },
            ControlMsg::PairOk {
                fingerprint: [7u8; 32],
            },
            ControlMsg::PairFail {
                code: 3001,
                reason: "PIN 错误".into(),
            },
            ControlMsg::StreamConfig(StreamConfig::default()),
            ControlMsg::RequestIdr,
            ControlMsg::SetBitrate { bps: 25_000_000 },
            ControlMsg::Feedback(Feedback {
                loss: 12,
                rtt: std::time::Duration::from_millis(3),
                recv_rate: 30_000_000,
            }),
            ControlMsg::ClockSync {
                t0: 1,
                t1: 2,
                t2: 3,
                t3: 4,
            },
            ControlMsg::StatsReport(StatsReport::default()),
            ControlMsg::Resume { session_id: 42 },
            ControlMsg::Bye,
            ControlMsg::Error { code: 2001 },
        ];
        for m in msgs {
            let mut buf = BytesMut::new();
            encode(&m, &mut buf).expect("编码应当成功");
            let back: ControlMsg = decode_as(&buf).expect("解码应当成功");
            assert_eq!(m, back);
        }
    }
}
