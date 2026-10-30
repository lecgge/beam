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
//! ## 与 mDNS 的关系
//!
//! §3.4 的 mDNS TXT 只广播指纹**前 8 位**，而信任库要的是完整 64 位十六进制。
//! 因此主机侧把「观察到的完整指纹」与「mDNS 广播的前缀」做**交叉校验**：
//! 前缀对不上就不生成配对请求（宁可少一条，不能错一条）。
//! 交叉校验由调用方（桌面端）用 mDNS 发现结果完成，
//! 本模块只负责「观察 → 记录 → 人工确认后放行」这条主线。

use std::sync::{Arc, Mutex};

use beam_pair::{Fingerprint, PeerVerifier, TrustStore};

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
}

impl HostTrustGate {
    /// 用共享信任库构造闸门。
    pub fn new(trust: Arc<Mutex<TrustStore>>) -> Self {
        Self {
            trust,
            observed: Mutex::new(Vec::new()),
            accepted: Mutex::new(None),
        }
    }

    /// 共享的信任库（调用方写库用）。
    pub fn trust(&self) -> Arc<Mutex<TrustStore>> {
        Arc::clone(&self.trust)
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
}

impl PeerVerifier for HostTrustGate {
    /// 证书校验：信任库里有就放行，没有就**拒绝并记住指纹**。
    ///
    /// 拒绝不是「遗忘」，而是「先不放行、等你人工确认」——
    /// 这正是产品方案第 102 行要求的安全环节。
    fn verify(&self, fp: &Fingerprint) -> bool {
        if self.is_trusted(fp) {
            let mut accepted = self.accepted.lock().unwrap_or_else(|e| e.into_inner());
            *accepted = Some(*fp);
            return true;
        }
        tracing::info!(
            fingerprint = %fp.short_hex(),
            "对端尚未写入信任库,拒绝本次握手并等待用户人工确认"
        );
        self.record(*fp);
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
}
