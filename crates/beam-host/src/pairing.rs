//! 主机侧信任闸门（§3.3 信任库 / 产品方案 §8 人工确认）。
//!
//! ## 为什么需要这一层
//!
//! 产品方案第 102 行把「主机端必须弹窗人工确认后才会写入信任库」
//! 列为**不可省略的安全环节**。落到代码上就是：
//!
//! 1. 客户端连过来时，主机侧**还不知道**它是谁 —— 信任库里没有它的指纹；
//! 2. 此时主机**不能**放行（那等于「自动放行」，正是产品方案禁止的）；
//! 3. 但主机必须**记住**这个对端的完整证书指纹，
//!    否则用户点了「同意」之后也没法把对端写进信任库；
//! 4. 用户在弹窗里点了「同意」→ 指纹写进信任库 → 下一次握手就被接受。
//!
//! 第 2 步与第 3 步是同一件事的两面：TLS 握手**故意失败**，
//! 而失败之前校验回调已经拿到了对端的完整指纹。
//! 这与客户端侧「用一次故意失败的握手采集对端指纹」是同一套办法。
//!
//! ## 两种模式
//!
//! | 模式 | 未信任对端的 TLS 握手 | 谁来把关 |
//! |---|---|---|
//! | **拒绝**（默认） | 拒绝并记住指纹 | 用户在弹窗里点「同意」→ 写信任库 → 下次握手才放行 |
//! | **配对** | **放行**（但记住指纹） | 控制面：SPAKE2 PIN / token 交换，失败就不准进串流 |
//!
//! 为什么需要「配对」模式：§3.3 的 PIN 流程要求客户端把 PIN 交给**主机**
//! 去校验，而 PIN 只能走**控制通道**送过去 —— 如果握手就被拒，
//! 控制通道根本开不了，PIN 也就没有用武之地。
//!
//! 因此配对模式下主机放行握手，但**安全兜底移到控制面**：
//! 会话层（`beam-session`）在 `PairingConfig::require_pairing` 为真时
//! **拒绝**未配对对端的 `Hello` —— 没配对就进不了串流。
//!
//! ## 与 mDNS 的关系
//!
//! §3.4 的 mDNS TXT 只广播指纹**前 8 位**，而信任库要的是完整 64 位十六进制。
//! 因此主机侧把「观察到的完整指纹」与「mDNS 广播的前缀」做**交叉校验**：
//! 前缀对不上就不生成配对请求（宁可少一条，不能错一条）。
//! 交叉校验由调用方（桌面端）用 mDNS 发现结果完成，
//! 本模块只负责「观察 → 记录 → 人工确认后放行」这条主线。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use beam_pair::{Fingerprint, PeerVerifier, TrustStore};

/// 主机侧**一次会话**的配对设置（§3.3）。
///
/// 为什么是「一次会话」而不是全局配置：PIN 每 `pin_ttl=120s` 轮换一次，
/// 而「这个对端要不要先配对」取决于**这一次握手**的对端是否在信任库里
/// （在 ⇒ TLS 层已放行，不需要再配对；不在 ⇒ 控制面必须先完成 PIN 交换）。
///
/// 主机编排层把它翻译成会话层的 [`beam_session::PairingConfig`]。
#[derive(Debug, Clone, Default)]
pub struct HostPairing {
    /// 主机显示的 PIN（客户端用它发起 SPAKE2，§3.3）。
    pub pin: Option<String>,
    /// 主机期望的一次性 token（`PairRequest` 路径，§5）。
    pub token: Option<String>,
    /// 本机证书指纹（放进 `PairOk` 与挑战载荷）。
    pub fingerprint: Option<Fingerprint>,
    /// 对端是否**必须**先完成配对才能进串流。
    ///
    /// `true` = 这一次握手的对端**不在信任库里**（TLS 层放行只是为了让它
    /// 能来做配对），控制面必须先完成 SPAKE2 / token 交换，
    /// 否则主机拒绝它的 `Hello`（`beam-session` 的控制层把关）。
    ///
    /// `false` = 对端已在信任库里（TLS 层已放行），不需要再配对。
    pub require_pairing: bool,
    /// 本次会话的**对端**指纹（来自 TLS 握手）。
    ///
    /// token 路径（§5 `PairRequest{token}`）要靠它：那条路的协议消息里
    /// 没有携带对端指纹的字段。
    pub peer: Option<Fingerprint>,
}

impl HostPairing {
    /// 按信任判定组装：需要配对时才把 `require_pairing` 置真。
    ///
    /// `verdict` 来自 [`HostTrustGate::take_verdict`]：
    /// - [`PeerVerdict::NeedsPairing`] ⇒ 要配对；
    /// - [`PeerVerdict::Trusted`] ⇒ 不用配对；
    /// - `None`（没有闸门 / 判定丢失）⇒ **保守地要求配对**
    ///   （宁可多要一次 PIN，也不放行一个没配对的对端）。
    pub fn for_verdict(
        verdict: Option<PeerVerdict>,
        pin: Option<String>,
        token: Option<String>,
        fingerprint: Option<Fingerprint>,
    ) -> Self {
        let require_pairing = !matches!(verdict, Some(PeerVerdict::Trusted(_)));
        let peer = match verdict {
            Some(PeerVerdict::Trusted(fp)) | Some(PeerVerdict::NeedsPairing(fp)) => Some(fp),
            None => None,
        };
        Self {
            pin,
            token,
            fingerprint,
            require_pairing,
            peer,
        }
    }
}

/// 一次握手对端的信任判定（§3.3）。
///
/// 主机接受连接后据此决定「这个对端要不要先完成配对」。
///
/// 判定里带着**对端指纹**：token 路径（§5 `PairRequest{token}`）的协议消息
/// 里没有携带对端指纹的字段，对端身份只能从 TLS 握手这里拿。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerVerdict {
    /// 已在信任库：TLS 层直接放行，**不需要**再配对。
    Trusted(Fingerprint),
    /// 未在信任库，但主机在**配对模式**：TLS 层放行握手，
    /// 控制面必须先完成配对（PIN / token）才准进串流。
    NeedsPairing(Fingerprint),
}

/// 一次「未信任对端」的观察记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedPeer {
    /// 对端证书指纹（完整 32 字节）。
    pub fingerprint: Fingerprint,
    /// 被拒绝的握手次数（对端在等用户确认时会反复重连）。
    pub attempts: u32,
}

impl ObservedPeer {
    /// 完整 64 位十六进制指纹（写信任库用的就是它）。
    pub fn fingerprint_hex(&self) -> String {
        self.fingerprint.to_hex()
    }

    /// 指纹前 8 位（与 mDNS TXT 的 `fp` 字段比对用）。
    pub fn short_hex(&self) -> String {
        self.fingerprint.short_hex()
    }
}

/// 主机侧信任闸门。
///
/// 实现了 [`PeerVerifier`]，因此可以直接交给 `beam_net::NetEndpoint::listen`：
/// 网络层只管「这张证书能不能过」，信任语义由本模块承担（§3.2 边界）。
///
/// 信任库用 `Arc<Mutex<TrustStore>>` 与调用方共享：
/// 用户点了「同意」之后调用方写库，闸门**立刻**就能看到（不需要重建监听）。
#[derive(Debug)]
pub struct HostTrustGate {
    /// 与调用方共享的信任库（§3.3）。
    trust: Arc<Mutex<TrustStore>>,
    /// 观察到的未信任对端（按指纹去重）。
    observed: Mutex<Vec<ObservedPeer>>,
    /// 最近一次被放行（已在信任库里）的对端指纹。
    accepted: Mutex<Option<Fingerprint>>,
    /// **配对模式**：未信任对端的握手也放行（控制面再把关）。
    pairing_mode: AtomicBool,
    /// 历次握手的信任判定（按握手顺序排队，accept 时依次取走）。
    verdicts: Mutex<VecDeque<PeerVerdict>>,
}

impl HostTrustGate {
    /// 用共享信任库构造闸门（默认「拒绝」模式）。
    pub fn new(trust: Arc<Mutex<TrustStore>>) -> Self {
        Self {
            trust,
            observed: Mutex::new(Vec::new()),
            accepted: Mutex::new(None),
            pairing_mode: AtomicBool::new(false),
            verdicts: Mutex::new(VecDeque::new()),
        }
    }

    /// 共享的信任库（调用方写库用）。
    pub fn trust(&self) -> Arc<Mutex<TrustStore>> {
        Arc::clone(&self.trust)
    }

    /// 进入 / 退出**配对模式**（§3.3 PIN 流程）。
    ///
    /// 配对模式下未信任对端的 TLS 握手**放行**（不然它连控制通道都到不了，
    /// PIN 送不过来）；安全兜底移到控制面：
    /// 会话层在 `require_pairing` 为真时拒绝未配对对端的 `Hello`。
    pub fn set_pairing_mode(&self, enabled: bool) {
        self.pairing_mode.store(enabled, Ordering::SeqCst);
        tracing::info!(
            enabled,
            "信任闸门切换配对模式(配对模式下放行未信任对端的握手,由控制面把关)"
        );
    }

    /// 当前是不是配对模式。
    pub fn is_pairing_mode(&self) -> bool {
        self.pairing_mode.load(Ordering::SeqCst)
    }

    /// 取走一次握手的信任判定（按握手顺序）。
    ///
    /// 主机接受连接后调用它，决定本次会话要不要**要求配对**。
    pub fn take_verdict(&self) -> Option<PeerVerdict> {
        self.verdicts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    /// 当前观察到的未信任对端（快照；不会清空）。
    ///
    /// 交叉校验可能失败（例如对端还没广播 mDNS），
    /// 因此这里**不**消费观察记录，由调用方在生成配对请求后显式清除。
    pub fn observed(&self) -> Vec<ObservedPeer> {
        self.observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 清除某个指纹的观察记录（已生成配对请求、或用户已拒绝时调用）。
    pub fn clear_observed(&self, fp: &Fingerprint) {
        let mut guard = self.observed.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|o| &o.fingerprint != fp);
    }

    /// 最近一次被放行的对端指纹（用于把「当前连上的设备」显示给用户）。
    pub fn last_accepted(&self) -> Option<Fingerprint> {
        *self.accepted.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 指纹是否已在信任库里。
    pub fn is_trusted(&self, fp: &Fingerprint) -> bool {
        self.trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_trusted(fp)
    }

    /// 记一次「未信任对端的握手尝试」。
    fn record(&self, fp: Fingerprint) {
        let mut guard = self.observed.lock().unwrap_or_else(|e| e.into_inner());
        match guard.iter_mut().find(|o| o.fingerprint == fp) {
            Some(existing) => existing.attempts = existing.attempts.saturating_add(1),
            None => guard.push(ObservedPeer {
                fingerprint: fp,
                attempts: 1,
            }),
        }
    }

    /// 记一次握手的信任判定。
    fn push_verdict(&self, verdict: PeerVerdict) {
        self.verdicts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(verdict);
    }
}

impl PeerVerifier for HostTrustGate {
    /// 证书校验：信任库里有就放行；没有就按当前模式决定。
    ///
    /// - **拒绝模式**（默认）：拒绝并记住指纹 —— 等用户人工确认（产品方案 §8）；
    /// - **配对模式**：放行握手，但控制面会要求先完成 PIN / token 交换。
    fn verify(&self, fp: &Fingerprint) -> bool {
        if self.is_trusted(fp) {
            let mut accepted = self.accepted.lock().unwrap_or_else(|e| e.into_inner());
            *accepted = Some(*fp);
            drop(accepted);
            self.push_verdict(PeerVerdict::Trusted(*fp));
            return true;
        }
        self.record(*fp);
        if self.pairing_mode.load(Ordering::SeqCst) {
            tracing::info!(
                fingerprint = %fp.short_hex(),
                "配对模式:放行未信任对端的握手,由控制面完成 PIN 校验"
            );
            self.push_verdict(PeerVerdict::NeedsPairing(*fp));
            return true;
        }
        tracing::info!(
            fingerprint = %fp.short_hex(),
            "对端尚未写入信任库,拒绝本次握手并等待用户人工确认"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_pair::TrustedPeer;

    fn store() -> Arc<Mutex<TrustStore>> {
        Arc::new(Mutex::new(TrustStore::new()))
    }

    #[test]
    fn untrusted_peer_is_rejected_and_observed() {
        let gate = HostTrustGate::new(store());
        let fp = Fingerprint::from_bytes([9u8; 32]);
        assert!(!gate.verify(&fp), "未信任的对端必须被拒绝");
        let observed = gate.observed();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].fingerprint, fp);
        assert_eq!(observed[0].attempts, 1);
        assert_eq!(observed[0].short_hex(), fp.short_hex());
    }

    #[test]
    fn repeated_handshakes_increment_attempts() {
        let gate = HostTrustGate::new(store());
        let fp = Fingerprint::from_bytes([1u8; 32]);
        assert!(!gate.verify(&fp));
        assert!(!gate.verify(&fp));
        assert_eq!(gate.observed()[0].attempts, 2);
    }

    #[test]
    fn approval_lets_the_peer_in_without_rebuilding_the_gate() {
        let trust = store();
        let gate = HostTrustGate::new(Arc::clone(&trust));
        let fp = Fingerprint::from_bytes([2u8; 32]);
        assert!(!gate.verify(&fp));

        // 用户点了「同意」→ 调用方写库。
        trust
            .lock()
            .unwrap()
            .upsert(TrustedPeer::new(fp, "手机", "android", 0));

        assert!(gate.verify(&fp), "写库之后必须放行");
        assert_eq!(gate.last_accepted(), Some(fp));
    }

    #[test]
    fn clear_observed_drops_the_record() {
        let gate = HostTrustGate::new(store());
        let fp = Fingerprint::from_bytes([3u8; 32]);
        assert!(!gate.verify(&fp));
        gate.clear_observed(&fp);
        assert!(gate.observed().is_empty());
    }

    #[test]
    fn pairing_mode_lets_the_untrusted_peer_through_the_handshake() {
        let gate = HostTrustGate::new(store());
        let fp = Fingerprint::from_bytes([4u8; 32]);
        // 默认是「拒绝」模式。
        assert!(!gate.is_pairing_mode());
        assert!(!gate.verify(&fp), "默认模式下未信任对端不得通过握手");

        // 切到配对模式 ⇒ 放行握手，但判定是「需要配对」。
        gate.set_pairing_mode(true);
        assert!(gate.is_pairing_mode());
        assert!(
            gate.verify(&fp),
            "配对模式下必须放行握手(否则 PIN 送不过来)"
        );
        assert_eq!(gate.take_verdict(), Some(PeerVerdict::NeedsPairing(fp)));
        // 指纹照样被记住（供用户确认 / 写信任库用）。
        assert_eq!(gate.observed().len(), 1);
        assert_eq!(gate.observed()[0].fingerprint, fp);
    }

    #[test]
    fn a_trusted_peer_needs_no_pairing() {
        let trust = store();
        let gate = HostTrustGate::new(Arc::clone(&trust));
        let fp = Fingerprint::from_bytes([5u8; 32]);
        trust
            .lock()
            .unwrap()
            .upsert(TrustedPeer::new(fp, "已配对设备", "windows", 0));
        assert!(gate.verify(&fp));
        assert_eq!(gate.take_verdict(), Some(PeerVerdict::Trusted(fp)));
    }

    #[test]
    fn a_rejected_handshake_produces_no_verdict() {
        // 「拒绝」模式下没有放行 ⇒ 不产生判定（主机不该为它开一个会话）。
        let gate = HostTrustGate::new(store());
        let fp = Fingerprint::from_bytes([6u8; 32]);
        assert!(!gate.verify(&fp));
        assert_eq!(gate.take_verdict(), None);
    }
}
