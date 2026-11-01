//! 控制通道的消息处理（§4.1 时序、§5 控制消息清单）。
//!
//! 本模块只做「消息 → 动作」的纯逻辑：**不含 IO、不含状态机**。
//! - 收消息、编解码：由 [`crate::event::ControlChannel`] 的实现负责（beam-client 的 QUIC 适配器）。
//! - 状态迁移：由 [`crate::client::ClientRunner`] / [`crate::host::HostRunner`] 负责。
//!
//! 覆盖 §4.1 时序里的控制面步骤：
//! `PairRequest` / `PairChallenge` → `PairOk` / `PairFail`、
//! `Hello` → 能力协商 → `HelloAck`、`RequestIdr`、`Feedback`、`Resume`、`Bye`。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use beam_pair::{client_challenge, host_challenge, Fingerprint, PinPairing};
use beam_proto::caps::feature;
use beam_proto::message::Hello;
use beam_proto::{
    negotiate, negotiate_version, Agreed, Caps, ControlMsg, ProtocolVersion, Resolution,
    StatsReport, StreamConfig,
};

use crate::clock::Clock;
use crate::error::SessionError;
use crate::event::RateControl;

/// 统计上报节拍（§3.11 `session://stats` 500 ms）。
pub const STATS_INTERVAL: Duration = Duration::from_millis(500);

/// 统计上报节拍（毫秒）。
pub const STATS_INTERVAL_MS: u64 = 500;

/// ClockSync 请求节拍（微秒）——§3.13「NTP 式四时间戳时钟同步，每 5 s 一次」。
pub const CLOCK_SYNC_INTERVAL_US: u64 = 5_000_000;

/// 生成会话号（§5 `Resume{session_id}` 复用）。
pub fn next_session_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// 控制通道角色（§4.1 时序的发起方 / 应答方）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRole {
    /// 客户端：发 `Hello`，收 `HelloAck`；发起配对。
    Client,
    /// 主机：收 `Hello`，回 `HelloAck`；应答配对。
    Host,
}

/// 配对配置（§3.3）。
///
/// 会话层不直接持有密钥材料，只持有「配对要用什么」：
/// - `pin`：客户端用它发起 SPAKE2，主机用它校验；
/// - `token`：主机期望的一次性 token（`PairRequest` 路径，§5）；
/// - `fingerprint`：本机证书指纹，用于 `PairOk` 与挑战载荷。
#[derive(Debug, Clone, Default)]
pub struct PairingConfig {
    /// PIN（客户端发起 / 主机校验）。
    pub pin: Option<String>,
    /// 主机期望的一次性 token（`PairRequest` 路径）。
    pub token: Option<String>,
    /// 本机证书指纹。
    pub fingerprint: Option<Fingerprint>,
}

/// 控制消息处理的结果（会话层内部使用）。
///
/// 处理器本身不改状态机，只把「应当做什么」交回给运行体。
#[derive(Debug, Default)]
pub struct ControlOutcome {
    /// 需要回复给对端的消息（按顺序发送）。
    pub replies: Vec<ControlMsg>,
    /// 需要请求 IDR（§4.1「重连后主机发 IDR」）。
    pub request_idr: bool,
    /// 会话应当关闭（收到 `Bye`）。
    pub close: bool,
    /// 协商结果（主机算出 / 客户端收到 `HelloAck`）。
    pub negotiated: Option<Agreed>,
    /// 要恢复的会话号（收到 `Resume`）。
    pub resumed: Option<u64>,
    /// 配对成功的对端指纹。
    pub paired: Option<Fingerprint>,
    /// 对端上报的统计。
    pub stats: Option<StatsReport>,
    /// 对端下发的串流配置。
    pub peer_config: Option<StreamConfig>,
    /// 对端要求的目标码率（bit/s）。
    pub bitrate_bps: Option<u32>,
    /// 处理失败。
    pub error: Option<SessionError>,
}

/// 控制消息处理器（§4.1 时序）。
pub struct ControlContext {
    role: ControlRole,
    local_caps: Caps,
    pairing: PairingConfig,
    /// 客户端侧 SPAKE2 状态（§3.3），发起配对后才有。
    client_pairing: Option<PinPairing>,
    /// 是否已经发起过配对（避免重复发 `PairRequest` / `PairChallenge`）。
    pairing_started: bool,
    /// 码率控制器（§3.2）。
    rate: Option<Box<dyn RateControl>>,
    /// 协商结果。
    agreed: Option<Agreed>,
    /// 时钟（`ClockSync` 需要真实时间戳）。
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ControlContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlContext")
            .field("role", &self.role)
            .field("local_caps", &self.local_caps)
            .field("pairing_started", &self.pairing_started)
            .field("agreed", &self.agreed)
            .finish()
    }
}

impl ControlContext {
    /// 构造处理器。
    pub fn new(
        role: ControlRole,
        local_caps: Caps,
        pairing: PairingConfig,
        rate: Option<Box<dyn RateControl>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            role,
            local_caps,
            pairing,
            client_pairing: None,
            pairing_started: false,
            rate,
            agreed: None,
            clock,
        }
    }

    /// 当前角色。
    pub fn role(&self) -> ControlRole {
        self.role
    }

    /// 本机能力集。
    pub fn local_caps(&self) -> &Caps {
        &self.local_caps
    }

    /// 协商结果（尚未协商时为 `None`）。
    pub fn agreed(&self) -> Option<&Agreed> {
        self.agreed.as_ref()
    }

    /// 发起配对（仅客户端，§4.1「`PairRequest`(token 或 SPAKE2 msg)」）。
    ///
    /// 配对消息先于 `Hello` 发出；收到 `PairOk` 之后调用方再发 `Hello`。
    /// 没有配置 PIN / token 时返回 `None`（表示无需配对）。
    pub fn start_pairing(&mut self) -> Option<ControlMsg> {
        if self.role != ControlRole::Client || self.pairing_started {
            return None;
        }
        self.pairing_started = true;

        // 优先走 token 路径（§5 `PairRequest{token}`）。
        if let Some(token) = self.pairing.token.clone() {
            return Some(ControlMsg::PairRequest { token });
        }

        let pin = self.pairing.pin.clone()?;
        let fingerprint = self.local_fingerprint();
        let (state, msg) = PinPairing::start(&pin);
        self.client_pairing = Some(state);
        Some(ControlMsg::PairChallenge {
            msg: client_challenge(&msg, fingerprint),
        })
    }

    /// 处理一条控制消息。
    pub fn handle(&mut self, msg: ControlMsg) -> ControlOutcome {
        let mut out = ControlOutcome::default();
        match msg {
            ControlMsg::Hello(hello) => {
                if self.role == ControlRole::Host {
                    self.handle_hello(hello, &mut out);
                }
            }
            ControlMsg::HelloAck { agreed } => {
                if self.role == ControlRole::Client {
                    self.agreed = Some(agreed.clone());
                    out.negotiated = Some(agreed);
                }
            }
            ControlMsg::PairRequest { token } => {
                if self.role == ControlRole::Host {
                    self.handle_pair_request(&token, &mut out);
                }
            }
            ControlMsg::PairChallenge { msg } => match self.role {
                ControlRole::Host => self.handle_host_challenge(&msg, &mut out),
                ControlRole::Client => self.handle_client_challenge(&msg, &mut out),
            },
            ControlMsg::PairOk { fingerprint } => {
                out.paired = Some(Fingerprint::from_bytes(fingerprint));
            }
            ControlMsg::PairFail { code, reason } => {
                out.error = Some(SessionError::TransportFailed {
                    reason: format!("配对失败(E{code}): {reason}"),
                });
            }
            ControlMsg::StreamConfig(cfg) => {
                out.peer_config = Some(cfg);
            }
            ControlMsg::RequestIdr => {
                out.request_idr = true;
            }
            ControlMsg::SetBitrate { bps } => {
                out.bitrate_bps = Some(bps);
            }
            ControlMsg::Feedback(fb) => {
                if let Some(rate) = self.rate.as_mut() {
                    out.bitrate_bps = Some(rate.on_feedback(&fb));
                }
            }
            ControlMsg::ClockSync { t0, .. } => {
                // §5：四个时间戳都是**微秒**（「所有时间戳为单调时钟微秒」）。
                // `t1` = 主机接收时刻，`t2` = 主机回复时刻。
                // 这里收到消息与生成回复是同一瞬间，因此两者相等。
                let now = self.clock.now_us();
                out.replies.push(ControlMsg::ClockSync {
                    t0,
                    t1: now,
                    t2: now,
                    t3: 0,
                });
            }
            ControlMsg::StatsReport(report) => {
                out.stats = Some(report);
            }
            ControlMsg::Resume { session_id } => {
                out.resumed = Some(session_id);
            }
            ControlMsg::Bye => {
                out.close = true;
            }
            ControlMsg::Error { code } => {
                out.error = Some(SessionError::TransportFailed {
                    reason: format!("对端报告错误(E{code})"),
                });
            }
        }
        out
    }

    /// 主机侧：处理 `Hello` —— 版本协商 + 能力协商（§3.1 / §5）。
    fn handle_hello(&mut self, hello: Hello, out: &mut ControlOutcome) {
        if let Err(e) = negotiate_version(&ProtocolVersion::CURRENT, &hello.ver) {
            out.replies.push(ControlMsg::Error { code: e.code() });
            out.error = Some(SessionError::TransportFailed {
                reason: format!("协议版本不兼容: {e}"),
            });
            return;
        }
        match negotiate(&self.local_caps, &hello.caps) {
            Ok(agreed) => {
                self.agreed = Some(agreed.clone());
                out.replies.push(ControlMsg::HelloAck {
                    agreed: agreed.clone(),
                });
                out.negotiated = Some(agreed);
            }
            Err(e) => {
                out.replies.push(ControlMsg::Error { code: e.code() });
                out.error = Some(SessionError::TransportFailed {
                    reason: format!("能力协商失败: {e}"),
                });
            }
        }
    }

    /// 主机侧：处理 `PairRequest{token}`（§5）。
    fn handle_pair_request(&mut self, token: &str, out: &mut ControlOutcome) {
        match self.pairing.token.as_deref() {
            Some(expected) if expected == token => {
                let fingerprint = self.local_fingerprint();
                out.replies.push(ControlMsg::PairOk {
                    fingerprint: *fingerprint.as_bytes(),
                });
                out.paired = Some(fingerprint);
            }
            _ => {
                out.replies.push(ControlMsg::PairFail {
                    code: 3003,
                    reason: "token 无效或已过期".to_string(),
                });
                out.error = Some(SessionError::TransportFailed {
                    reason: "配对失败(E3003): token 无效或已过期".to_string(),
                });
            }
        }
    }

    /// 主机侧：校验客户端的 SPAKE2 首条消息并应答（§3.3）。
    fn handle_host_challenge(&mut self, challenge: &[u8], out: &mut ControlOutcome) {
        let Some(pin) = self.pairing.pin.clone() else {
            out.replies.push(ControlMsg::PairFail {
                code: 3001,
                reason: "主机未配置 PIN".to_string(),
            });
            out.error = Some(SessionError::TransportFailed {
                reason: "配对失败(E3001): 主机未配置 PIN".to_string(),
            });
            return;
        };
        match PinPairing::respond(challenge, &pin) {
            Ok((outcome, reply)) => {
                let fingerprint = self.local_fingerprint();
                out.replies.push(ControlMsg::PairChallenge {
                    msg: host_challenge(&reply, fingerprint, &outcome.shared_key),
                });
                // 对端（客户端）的指纹由挑战载荷带出。
                out.paired = Some(outcome.fingerprint);
            }
            Err(e) => {
                out.replies.push(ControlMsg::PairFail {
                    code: e.code(),
                    reason: e.to_string(),
                });
                out.error = Some(SessionError::TransportFailed {
                    reason: format!("配对失败(E{}): {e}", e.code()),
                });
            }
        }
    }

    /// 客户端侧：完成 SPAKE2 交换并回 `PairOk`（§3.3）。
    fn handle_client_challenge(&mut self, challenge: &[u8], out: &mut ControlOutcome) {
        let Some(state) = self.client_pairing.take() else {
            out.replies.push(ControlMsg::PairFail {
                code: 3001,
                reason: "客户端尚未发起配对".to_string(),
            });
            out.error = Some(SessionError::TransportFailed {
                reason: "配对失败(E3001): 客户端尚未发起配对".to_string(),
            });
            return;
        };
        match state.finish(challenge) {
            Ok(outcome) => {
                out.replies.push(ControlMsg::PairOk {
                    fingerprint: *outcome.fingerprint.as_bytes(),
                });
                out.paired = Some(outcome.fingerprint);
            }
            Err(e) => {
                out.replies.push(ControlMsg::PairFail {
                    code: e.code(),
                    reason: e.to_string(),
                });
                out.error = Some(SessionError::TransportFailed {
                    reason: format!("配对失败(E{}): {e}", e.code()),
                });
            }
        }
    }

    fn local_fingerprint(&self) -> Fingerprint {
        self.pairing
            .fingerprint
            .unwrap_or_else(|| Fingerprint::from_bytes([0u8; Fingerprint::LEN]))
    }
}

/// 由串流配置推导能力集（§3.1 `Caps`）。
pub fn caps_from_config(cfg: &StreamConfig) -> Caps {
    let mut features = cfg.features;
    if cfg.audio_enabled {
        features |= feature::AUDIO;
    }
    Caps {
        codecs: vec![cfg.codec],
        max_res: Resolution::new(cfg.width, cfg.height),
        max_fps: cfg.fps,
        features,
    }
}

/// 由协商结果生成串流配置（§3.1 `Agreed` → `StreamConfig`）。
pub fn config_from_agreed(agreed: &Agreed, base: &StreamConfig) -> StreamConfig {
    StreamConfig {
        width: agreed.width,
        height: agreed.height,
        fps: agreed.fps,
        bitrate_bps: base.bitrate_bps,
        codec: agreed.codec,
        audio_enabled: agreed.features & feature::AUDIO != 0,
        features: agreed.features,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use beam_proto::Feedback;

    fn caps() -> Caps {
        Caps {
            codecs: vec![beam_proto::CodecKind::H264, beam_proto::CodecKind::Hevc],
            max_res: Resolution::new(1920, 1080),
            max_fps: 60,
            features: feature::AUDIO,
        }
    }

    fn ctx(role: ControlRole) -> ControlContext {
        ControlContext::new(
            role,
            caps(),
            PairingConfig::default(),
            None,
            Arc::new(ManualClock::new()),
        )
    }

    #[test]
    fn host_answers_hello_with_negotiated_caps() {
        let mut host = ctx(ControlRole::Host);
        let remote = Caps {
            codecs: vec![beam_proto::CodecKind::H264],
            max_res: Resolution::new(1280, 720),
            max_fps: 30,
            features: feature::AUDIO,
        };
        let out = host.handle(ControlMsg::Hello(Hello {
            ver: ProtocolVersion::CURRENT,
            caps: remote,
        }));
        assert!(out.error.is_none());
        assert_eq!(out.replies.len(), 1);
        match &out.replies[0] {
            ControlMsg::HelloAck { agreed } => {
                assert_eq!(agreed.codec, beam_proto::CodecKind::H264);
                assert_eq!(agreed.width, 1280);
                assert_eq!(agreed.height, 720);
                assert_eq!(agreed.fps, 30);
                assert_eq!(agreed.features, feature::AUDIO);
            }
            other => panic!("期望 HelloAck,实际 {other:?}"),
        }
        assert!(out.negotiated.is_some());
    }

    #[test]
    fn host_rejects_incompatible_version() {
        let mut host = ctx(ControlRole::Host);
        let out = host.handle(ControlMsg::Hello(Hello {
            ver: ProtocolVersion { major: 2, minor: 0 },
            caps: caps(),
        }));
        assert!(out.error.is_some());
        assert!(matches!(out.replies[0], ControlMsg::Error { code: 1001 }));
    }

    #[test]
    fn host_rejects_capability_mismatch() {
        let mut host = ctx(ControlRole::Host);
        let remote = Caps {
            codecs: vec![beam_proto::CodecKind::Av1],
            ..caps()
        };
        let out = host.handle(ControlMsg::Hello(Hello {
            ver: ProtocolVersion::CURRENT,
            caps: remote,
        }));
        assert!(out.error.is_some());
        assert!(matches!(out.replies[0], ControlMsg::Error { code: 1004 }));
    }

    #[test]
    fn host_rejects_wrong_pair_token() {
        let mut host = ControlContext::new(
            ControlRole::Host,
            caps(),
            PairingConfig {
                token: Some("right".into()),
                ..PairingConfig::default()
            },
            None,
            Arc::new(ManualClock::new()),
        );
        let out = host.handle(ControlMsg::PairRequest {
            token: "wrong".into(),
        });
        match &out.replies[0] {
            ControlMsg::PairFail { code, .. } => assert_eq!(*code, 3003),
            other => panic!("期望 PairFail,实际 {other:?}"),
        }
        assert!(out.error.is_some());
    }

    #[test]
    fn host_accepts_right_pair_token() {
        let mut host = ControlContext::new(
            ControlRole::Host,
            caps(),
            PairingConfig {
                token: Some("right".into()),
                fingerprint: Some(Fingerprint::from_bytes([7u8; 32])),
                ..PairingConfig::default()
            },
            None,
            Arc::new(ManualClock::new()),
        );
        let out = host.handle(ControlMsg::PairRequest {
            token: "right".into(),
        });
        match &out.replies[0] {
            ControlMsg::PairOk { fingerprint } => assert_eq!(*fingerprint, [7u8; 32]),
            other => panic!("期望 PairOk,实际 {other:?}"),
        }
        assert!(out.paired.is_some());
    }

    #[test]
    fn spake2_round_trip_produces_pair_ok() {
        let client_fp = Fingerprint::from_bytes([1u8; 32]);
        let host_fp = Fingerprint::from_bytes([2u8; 32]);

        let mut client = ControlContext::new(
            ControlRole::Client,
            caps(),
            PairingConfig {
                pin: Some("1234".into()),
                fingerprint: Some(client_fp),
                ..PairingConfig::default()
            },
            None,
            Arc::new(ManualClock::new()),
        );
        let mut host = ControlContext::new(
            ControlRole::Host,
            caps(),
            PairingConfig {
                pin: Some("1234".into()),
                fingerprint: Some(host_fp),
                ..PairingConfig::default()
            },
            None,
            Arc::new(ManualClock::new()),
        );

        let first = client.start_pairing().expect("应当发起配对");
        let ControlMsg::PairChallenge { msg } = first else {
            panic!("期望 PairChallenge");
        };
        let reply = host.handle(ControlMsg::PairChallenge { msg });
        let ControlMsg::PairChallenge { msg } = reply.replies[0].clone() else {
            panic!("期望主机回 PairChallenge");
        };
        let done = client.handle(ControlMsg::PairChallenge { msg });
        match &done.replies[0] {
            ControlMsg::PairOk { fingerprint } => assert_eq!(*fingerprint, *host_fp.as_bytes()),
            other => panic!("期望 PairOk,实际 {other:?}"),
        }
        assert_eq!(done.paired, Some(host_fp));
    }

    #[test]
    fn wrong_pin_produces_pair_fail() {
        let mut client = ControlContext::new(
            ControlRole::Client,
            caps(),
            PairingConfig {
                pin: Some("1234".into()),
                fingerprint: Some(Fingerprint::from_bytes([1u8; 32])),
                ..PairingConfig::default()
            },
            None,
            Arc::new(ManualClock::new()),
        );
        let mut host = ControlContext::new(
            ControlRole::Host,
            caps(),
            PairingConfig {
                pin: Some("9999".into()),
                fingerprint: Some(Fingerprint::from_bytes([2u8; 32])),
                ..PairingConfig::default()
            },
            None,
            Arc::new(ManualClock::new()),
        );
        let ControlMsg::PairChallenge { msg } = client.start_pairing().expect("应当发起配对")
        else {
            panic!("期望 PairChallenge");
        };
        // 主机侧 SPAKE2 本身不会发现 PIN 不一致（§3.3）：它照常应答，
        // 由客户端在密钥确认阶段发现并回 `PairFail`。
        let reply = host.handle(ControlMsg::PairChallenge { msg });
        let ControlMsg::PairChallenge { msg } = reply.replies[0].clone() else {
            panic!("期望主机回 PairChallenge,实际 {:?}", reply.replies[0]);
        };
        let done = client.handle(ControlMsg::PairChallenge { msg });
        assert!(
            matches!(done.replies[0], ControlMsg::PairFail { code: 3001, .. }),
            "PIN 不一致必须回 PairFail(E3001),实际 {:?}",
            done.replies[0]
        );
        assert!(done.error.is_some());
    }

    #[test]
    fn feedback_is_forwarded_to_rate_controller() {
        struct Recorder {
            seen: Vec<u32>,
        }
        impl RateControl for Recorder {
            fn on_feedback(&mut self, fb: &Feedback) -> u32 {
                self.seen.push(fb.loss);
                12_000_000
            }
        }
        let mut host = ControlContext::new(
            ControlRole::Host,
            caps(),
            PairingConfig::default(),
            Some(Box::new(Recorder { seen: Vec::new() })),
            Arc::new(ManualClock::new()),
        );
        let out = host.handle(ControlMsg::Feedback(Feedback {
            loss: 42,
            rtt: std::time::Duration::from_millis(5),
            recv_rate: 30_000_000,
        }));
        assert_eq!(out.bitrate_bps, Some(12_000_000));
    }

    #[test]
    fn resume_and_bye_are_surfaced() {
        let mut host = ctx(ControlRole::Host);
        let out = host.handle(ControlMsg::Resume { session_id: 42 });
        assert_eq!(out.resumed, Some(42));
        let out = host.handle(ControlMsg::Bye);
        assert!(out.close);
    }

    #[test]
    fn caps_from_config_marks_audio_feature() {
        let cfg = StreamConfig {
            audio_enabled: true,
            ..StreamConfig::default()
        };
        let caps = caps_from_config(&cfg);
        assert!(caps.has_feature(feature::AUDIO));
        assert_eq!(caps.max_fps, cfg.fps);
    }

    #[test]
    fn config_from_agreed_keeps_base_bitrate() {
        let agreed = Agreed {
            codec: beam_proto::CodecKind::Hevc,
            width: 2560,
            height: 1440,
            fps: 120,
            features: feature::AUDIO,
        };
        let base = StreamConfig {
            bitrate_bps: 42_000_000,
            ..StreamConfig::default()
        };
        let cfg = config_from_agreed(&agreed, &base);
        assert_eq!(cfg.width, 2560);
        assert_eq!(cfg.fps, 120);
        assert_eq!(cfg.bitrate_bps, 42_000_000);
        assert!(cfg.audio_enabled);
    }
}
