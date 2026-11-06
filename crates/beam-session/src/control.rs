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
/// - `peer`：本次会话的**对端**指纹（主机侧，由 TLS 层给出）；
/// - `fingerprint`：本机证书指纹，用于 `PairOk` 与挑战载荷；
/// - `require_pairing`：主机侧「对端**必须**先完成配对才准进串流」。
#[derive(Debug, Clone, Default)]
pub struct PairingConfig {
    /// PIN（客户端发起 / 主机校验）。
    pub pin: Option<String>,
    /// 主机期望的一次性 token（`PairRequest` 路径）。
    pub token: Option<String>,
    /// 本次会话的**对端**指纹（主机侧；由 TLS 层给出）。
    ///
    /// token 路径（`PairRequest{token}`）用得到：那条路上协议里没有
    /// 携带对端指纹的字段，对端身份只能从 TLS 握手那里拿。
    /// SPAKE2 路径则用挑战载荷里带的指纹（更严格：它跟着密码学证明一起过来）。
    pub peer: Option<Fingerprint>,
    /// 本机证书指纹。
    pub fingerprint: Option<Fingerprint>,
    /// 主机侧：未完成配对的对端**不得**进入串流（§3.3）。
    ///
    /// 为什么需要它：主机在「配对模式」下会**放行**未信任对端的 TLS 握手
    /// （不然对端连控制通道都到不了，PIN 就没有用武之地），
    /// 因此**控制面**必须自己把关：SPAKE2 / token 交换没成功之前，
    /// 一律不接受 `Hello`。
    ///
    /// 已经在信任库里的对端走的是另一条路（TLS 层直接放行），
    /// 那时主机把本字段置 `false`（见 `beam_host::HostPairing`）。
    pub require_pairing: bool,
}

/// §3.3：“尚未完成配对（等待用户人工确认）”的错误码。
///
/// 主机在信任闸门的「配对模式」下会放行未受信任对端的握手（不然 PIN 送不到主机），
/// 但控制面会要求完成配对才能进串流。用户在主机弹窗里点「同意」之前，
/// 客户端收到的就是这个码 —— 它不是「PIN 错了」，而是「还没点同意」。
///
/// 客户端据此进入「等待确认」而不是直接失败（客户端重连一次就能进）。
pub const PAIRING_PENDING_CODE: u16 = 3006;

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
    /// 对端说「还没配对好，等用户在主机上点同意」。
    ///
    /// 这不是错误，是「等一下」—— 客户端应该等待并重连。
    pub pairing_pending: bool,
}

/// 控制消息处理器（§4.1 时序）。
pub struct ControlContext {
    role: ControlRole,
    local_caps: Caps,
    pairing: PairingConfig,
    /// 客户端侧 SPAKE2 状态（§3.3），发起配对后才有。
    client_pairing: Option<PinPairing>,
    /// 主机侧：**已完成配对**的对端指纹（§3.3）。
    ///
    /// `require_pairing` 为真时，这个字段是 `None` 就表示「还没配对」——
    /// 此时主机**拒绝** `Hello`（见 [`ControlContext::handle_hello`]）。
    paired: Option<Fingerprint>,
    /// 主机侧：客户端在挑战载荷里带来的**对端指纹**（还没证明它知道 PIN）。
    ///
    /// SPAKE2 的密码学证明在**客户端**那一步才成立（客户端拿自己的 PIN
    /// 校验主机的应答，对不上就发 `PairFail`）。所以主机**不能**在
    /// 「收到首条挑战」时就认定配对成功 —— 那样一个**输错 PIN** 的客户端
    /// 也会被写进信任库。
    ///
    /// 主机的做法是：收到挑战时先记住对端指纹（`pending_peer`），
    /// 等客户端回 `PairOk`（= 它真的过了 PIN 校验）才把配对坐实。
    pending_peer: Option<Fingerprint>,
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
            paired: None,
            pending_peer: None,
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

    /// 重置配对状态，让下一次 `start_pairing` 能再次发起配对。
    ///
    /// 用在「等主机人工确认」的重连路径：主机上一轮已经拒绝过，
    /// 客户端重连后必须能**重新**u53d1起一次 SPAKE2 交换（上一次的状态已被消费）。
    pub fn reset_pairing(&mut self) {
        self.client_pairing = None;
        self.pairing_started = false;
    }

    /// 已完成配对的对端指纹（`None` = 还没配对）。
    ///
    /// 主机侧用它判断「能不能进串流」：`require_pairing` 为真且这里是 `None`
    /// ⇒ 拒绝 `Hello`。
    pub fn paired(&self) -> Option<Fingerprint> {
        self.paired
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
                let fp = Fingerprint::from_bytes(fingerprint);
                match self.role {
                    // 主机：客户端的 `PairOk` 表示**它真的过了 PIN 校验**
                    // （SPAKE2 的密码学证明在客户端那一步才成立）。
                    // 对端指纹取自挑战载荷 —— 客户端在 `PairOk` 里带的是
                    // **主机**的指纹，对主机自己没用。
                    ControlRole::Host => {
                        if let Some(peer) = self.pending_peer {
                            self.paired = Some(peer);
                            out.paired = Some(peer);
                        }
                    }
                    ControlRole::Client => {
                        self.paired = Some(fp);
                        out.paired = Some(fp);
                    }
                }
            }
            ControlMsg::PairFail { code, reason } => {
                if code == PAIRING_PENDING_CODE {
                    // 主机还没得到人工确认 → 这不是错误，是「等一下」。
                    out.pairing_pending = true;
                } else {
                    out.error = Some(SessionError::TransportFailed {
                        reason: format!("配对失败(E{code}): {reason}"),
                    });
                }
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

    /// 主机侧：处理 `Hello` —— 配对把关 + 版本协商 + 能力协商（§3.1 / §5）。
    fn handle_hello(&mut self, hello: Hello, out: &mut ControlOutcome) {
        // §3.3：主机在「配对模式」下会放行未信任对端的 TLS 握手
        // （不然对端连控制通道都到不了，PIN 没有用武之地），
        // 因此**控制面**必须自己把关：配对没成功之前一律不接受 `Hello`。
        if self.role == ControlRole::Host && self.pairing.require_pairing && self.paired.is_none() {
            out.replies.push(ControlMsg::PairFail {
                code: PAIRING_PENDING_CODE,
                reason: "尚未完成配对,等待用户在主机上确认".to_string(),
            });
            out.error = Some(SessionError::TransportFailed {
                reason: format!(
                    "配对待确认(E{PAIRING_PENDING_CODE}): 尚未完成配对,等待用户在主机上确认"
                ),
            });
            return;
        }
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
                // 配对完成（§3.3）：token 本身就是客户端「知道一次性口令」的证明，
                // 所以这条路上**当场**就能坐实。
                // 对端指纹取自 TLS 层（协议里没有携带对端指纹的字段）。
                self.paired = self.pairing.peer;
                out.paired = self.paired;
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
                // **先只记着**，不坐实配对：SPAKE2 的密码学证明在**客户端**
                // 那一步才成立（客户端拿自己的 PIN 校验主机的应答，
                // 对不上就回 `PairFail`）。坐实发生在收到客户端的 `PairOk` 时。
                self.pending_peer = Some(outcome.fingerprint);
                out.paired = None;
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
                // token 路径的对端指纹来自 TLS 层（协议消息里没有这个字段）。
                peer: Some(Fingerprint::from_bytes([8u8; 32])),
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
        assert_eq!(
            out.paired,
            Some(Fingerprint::from_bytes([8u8; 32])),
            "token 路径：配对坐实,对端指纹取自 TLS 层"
        );
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

    /// 主机侧要求配对时，**没配对就不许进串流**（§3.3）。
    ///
    /// 这是「配对模式放行未信任对端握手」的安全兜底：
    /// 握手放行了，但控制面必须自己把关。
    #[test]
    fn host_rejects_hello_when_pairing_required_but_not_done() {
        let mut host = ControlContext::new(
            ControlRole::Host,
            caps(),
            PairingConfig {
                pin: Some("1234".into()),
                fingerprint: Some(Fingerprint::from_bytes([2u8; 32])),
                token: None,
                peer: None,
                require_pairing: true,
            },
            None,
            Arc::new(ManualClock::new()),
        );
        let out = host.handle(ControlMsg::Hello(Hello {
            ver: ProtocolVersion::CURRENT,
            caps: caps(),
        }));
        assert!(out.error.is_some(), "未配对的主机不得接受 Hello");
        assert!(out.negotiated.is_none(), "未配对时不得完成能力协商");
        match &out.replies[0] {
            // 「还没配对好」不是「PIN 错了」，要用一个能区分的码（E3006）。
            ControlMsg::PairFail { code, .. } => assert_eq!(*code, PAIRING_PENDING_CODE),
            other => panic!("期望 PairFail,实际 {other:?}"),
        }
    }

    /// 配对成功之后，主机就准这个对端进串流（§3.3）。
    #[test]
    fn host_accepts_hello_after_successful_spake2_pairing() {
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
                token: None,
                peer: None,
                require_pairing: true,
            },
            None,
            Arc::new(ManualClock::new()),
        );

        // 完整跑一遍 SPAKE2：客户端发起 → 主机应答 → 客户端回 PairOk。
        let ControlMsg::PairChallenge { msg } = client.start_pairing().expect("应当发起配对")
        else {
            panic!("期望 PairChallenge");
        };
        let reply = host.handle(ControlMsg::PairChallenge { msg });
        // 主机此时**还没有**坐实配对 —— SPAKE2 的密码学证明在客户端那一步
        // 才成立（客户端拿自己的 PIN 校验主机的应答）。
        assert_eq!(
            host.paired(),
            None,
            "收到首条挑战不等于配对成功：主机还不知道客户端是否知道 PIN"
        );
        let ControlMsg::PairChallenge { msg } = reply.replies[0].clone() else {
            panic!("期望主机回 PairChallenge");
        };
        let done = client.handle(ControlMsg::PairChallenge { msg });
        let pair_ok = done
            .replies
            .iter()
            .find_map(|m| match m {
                ControlMsg::PairOk { fingerprint } => Some(*fingerprint),
                _ => None,
            })
            .expect("客户端应当回 PairOk");

        // 客户端回了 `PairOk` ⇒ 它真的过了 PIN 校验 ⇒ 主机这才坐实配对。
        let host_done = host.handle(ControlMsg::PairOk {
            fingerprint: pair_ok,
        });
        assert_eq!(host.paired(), Some(client_fp));
        assert_eq!(host_done.paired, Some(client_fp));

        // 于是 `Hello` 被接受。
        let out = host.handle(ControlMsg::Hello(Hello {
            ver: ProtocolVersion::CURRENT,
            caps: caps(),
        }));
        assert!(out.error.is_none(), "配对成功后不得再拒绝 Hello");
        assert!(out.negotiated.is_some());
    }

    /// **安全回归**：PIN 输错时，主机**不得**把对端坐实为「已配对」。
    ///
    /// SPAKE2 的密码学证明在**客户端**那一步才成立：客户端拿自己的 PIN
    /// 校验主机的应答，对不上就回 `PairFail`。
    /// 主机若在「收到首条挑战」时就坐实配对（并写信任库），
    /// 一个**输错 PIN** 的客户端也会被当成已配对设备 —— 这正是本测试要挡住的。
    #[test]
    fn host_does_not_confirm_pairing_before_the_client_proves_it() {
        let client_fp = Fingerprint::from_bytes([1u8; 32]);
        let host_fp = Fingerprint::from_bytes([2u8; 32]);

        let mut client = ControlContext::new(
            ControlRole::Client,
            caps(),
            PairingConfig {
                pin: Some("9999".into()), // 故意输错
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
                token: None,
                peer: None,
                require_pairing: true,
            },
            None,
            Arc::new(ManualClock::new()),
        );

        let ControlMsg::PairChallenge { msg } = client.start_pairing().expect("应当发起配对")
        else {
            panic!("期望 PairChallenge");
        };
        let reply = host.handle(ControlMsg::PairChallenge { msg });
        // 主机只是**记下了**对端指纹，还没有坐实。
        assert_eq!(host.paired(), None);

        let ControlMsg::PairChallenge { msg } = reply.replies[0].clone() else {
            panic!("期望主机回 PairChallenge");
        };
        let done = client.handle(ControlMsg::PairChallenge { msg });
        // 客户端发现 PIN 对不上 ⇒ 回 `PairFail`，**不会**回 `PairOk`。
        assert!(
            done.replies
                .iter()
                .any(|m| matches!(m, ControlMsg::PairFail { .. })),
            "PIN 对不上时客户端必须回 PairFail"
        );
        assert!(
            !done
                .replies
                .iter()
                .any(|m| matches!(m, ControlMsg::PairOk { .. })),
            "PIN 对不上时客户端绝不能回 PairOk"
        );
        // 主机因此始终没有坐实配对 ⇒ 之后写不进信任库。
        assert_eq!(host.paired(), None);
    }

    /// 不要求配对时（默认路径），`Hello` 的行为**不变**（回归保护）。
    #[test]
    fn host_accepts_hello_when_pairing_not_required() {
        let mut host = ctx(ControlRole::Host);
        let out = host.handle(ControlMsg::Hello(Hello {
            ver: ProtocolVersion::CURRENT,
            caps: caps(),
        }));
        assert!(out.error.is_none());
        assert!(out.negotiated.is_some());
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
