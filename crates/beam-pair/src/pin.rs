//! SPAKE2 PIN 配对流程（§3.3 `PinPairing::start/finish`）。
//!
//! 流程（§5 `PairRequest` / `PairChallenge` / `PairOk` / `PairFail`）：
//!
//! 1. 客户端 `PinPairing::start(pin)` 得到首条消息，发 `PairChallenge`；
//! 2. 主机 `PinPairing::respond(challenge, pin)` 校验并得到应答消息；
//! 3. 客户端 `PinPairing::finish(challenge)` 校验密钥确认值，得到共享密钥与主机指纹；
//! 4. 双方用共享密钥派生的确认值互相确认，不一致即 `E3001`（PIN 错误）。
//!
//! 配置（§3.3）：`pin_len=4`、`pin_ttl=120s`、`max_attempts=3`。

use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::error::PairError;
use crate::fingerprint::Fingerprint;

/// 客户端侧 SPAKE2 身份串。
pub const ID_CLIENT: &[u8] = b"beam-client";
/// 主机侧 SPAKE2 身份串。
pub const ID_HOST: &[u8] = b"beam-host";
/// 密钥确认标签（主机侧）。
const CONFIRM_LABEL: &[u8] = b"beam-pair-confirm-v1-b";

type HmacSha256 = Hmac<Sha256>;

/// 配对挑战载荷（承载在 §5 `PairChallenge` 的 `msg` 字段内）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairChallenge {
    /// SPAKE2 首条 / 应答消息。
    pub spake2: Vec<u8>,
    /// 发送方证书指纹。
    pub fingerprint: Fingerprint,
    /// 密钥确认值（仅应答方填写）。
    #[serde(default)]
    pub confirm: Vec<u8>,
}

/// 配对结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinOutcome {
    /// 对端证书指纹（§3.3 的 `finish` 结果）。
    pub fingerprint: Fingerprint,
    /// SPAKE2 派生的共享密钥，用于后续消息的完整性校验。
    pub shared_key: Vec<u8>,
}

/// 配对策略（§3.3 配置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingPolicy {
    /// PIN 长度（§3.3 `pin_len=4`）。
    pub pin_len: usize,
    /// 一次性 token 有效期（§3.3 `pin_ttl=120s`）。
    pub pin_ttl: Duration,
    /// 最大尝试次数（§3.3 `max_attempts=3`）。
    pub max_attempts: u32,
    /// 超限锁定时长（§3.3 `E3002` 锁定 60 s）。
    pub lockout: Duration,
}

impl Default for PairingPolicy {
    fn default() -> Self {
        Self {
            pin_len: 4,
            pin_ttl: Duration::from_secs(120),
            max_attempts: 3,
            lockout: Duration::from_secs(60),
        }
    }
}

/// 一次性 token（§3.3 `pin_ttl=120s`、§5 `PairRequest{token}`）。
#[derive(Debug, Clone)]
pub struct PairingToken {
    value: String,
    issued_at: Instant,
    ttl: Duration,
}

impl PairingToken {
    /// 签发 token。
    pub fn issue(ttl: Duration) -> Self {
        Self {
            value: crate::identity::generate_token(),
            issued_at: Instant::now(),
            ttl,
        }
    }

    /// token 字符串。
    pub fn value(&self) -> &str {
        &self.value
    }

    /// 校验 token 是否过期（过期 → `E3003`）。
    pub fn verify(&self, now: Instant) -> Result<(), PairError> {
        if now.duration_since(self.issued_at) > self.ttl {
            return Err(PairError::TokenExpired);
        }
        Ok(())
    }
}

/// 尝试次数守卫（§3.3 `E3002`：超过 `max_attempts` 锁定 60 s）。
#[derive(Debug)]
pub struct AttemptGuard {
    policy: PairingPolicy,
    failures: u32,
    locked_until: Option<Instant>,
}

impl AttemptGuard {
    /// 用给定策略构造。
    pub fn new(policy: PairingPolicy) -> Self {
        Self {
            policy,
            failures: 0,
            locked_until: None,
        }
    }

    /// 剩余尝试次数。
    pub fn remaining_attempts(&self) -> u32 {
        self.policy.max_attempts.saturating_sub(self.failures)
    }

    /// 是否处于锁定期。
    pub fn is_locked(&self, now: Instant) -> bool {
        matches!(self.locked_until, Some(until) if now < until)
    }

    /// 检查是否可以继续尝试（锁定中 → `E3002`）。
    pub fn check(&self, now: Instant) -> Result<(), PairError> {
        if self.is_locked(now) {
            return Err(PairError::AttemptsExceeded {
                max_attempts: self.policy.max_attempts,
                lockout_secs: self.policy.lockout.as_secs(),
            });
        }
        Ok(())
    }

    /// 记录一次失败；超过上限则锁定并返回 `E3002`。
    pub fn record_failure(&mut self, now: Instant) -> Result<(), PairError> {
        self.failures += 1;
        if self.failures >= self.policy.max_attempts {
            self.locked_until = Some(now + self.policy.lockout);
            return Err(PairError::AttemptsExceeded {
                max_attempts: self.policy.max_attempts,
                lockout_secs: self.policy.lockout.as_secs(),
            });
        }
        Ok(())
    }

    /// 成功后清零。
    pub fn record_success(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }
}

/// SPAKE2 PIN 配对会话（§3.3）。
pub struct PinPairing {
    state: Spake2<Ed25519Group>,
}

impl std::fmt::Debug for PinPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PinPairing { .. }")
    }
}

impl PinPairing {
    /// 发起方：用 PIN 生成首条消息（§3.3 `PinPairing::start(pin)`）。
    pub fn start(pin: &str) -> (Self, Vec<u8>) {
        let (state, msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(pin.as_bytes()),
            &Identity::new(ID_CLIENT),
            &Identity::new(ID_HOST),
        );
        (Self { state }, msg)
    }

    /// 响应方（主机）：校验客户端首条消息并生成应答（§5 `PairChallenge`）。
    ///
    /// 返回 `(配对结果, 应答消息)`；结果里的共享密钥用于生成密钥确认值。
    pub fn respond(challenge: &[u8], pin: &str) -> Result<(PinOutcome, Vec<u8>), PairError> {
        let challenge: PairChallenge =
            postcard::from_bytes(challenge).map_err(|_| PairError::MalformedChallenge)?;
        let (state, reply) = Spake2::<Ed25519Group>::start_b(
            &Password::new(pin.as_bytes()),
            &Identity::new(ID_CLIENT),
            &Identity::new(ID_HOST),
        );
        let shared_key = state
            .finish(&challenge.spake2)
            .map_err(|_| PairError::PinIncorrect)?;
        let outcome = PinOutcome {
            fingerprint: challenge.fingerprint,
            shared_key: shared_key.clone(),
        };
        Ok((outcome, reply))
    }

    /// 发起方完成交换（§3.3 `finish(msg)`）。
    ///
    /// 校验对方的密钥确认值；不一致说明 PIN 不同 → `E3001`。
    pub fn finish(self, challenge: &[u8]) -> Result<PinOutcome, PairError> {
        let challenge: PairChallenge =
            postcard::from_bytes(challenge).map_err(|_| PairError::MalformedChallenge)?;
        let shared_key = self
            .state
            .finish(&challenge.spake2)
            .map_err(|_| PairError::PinIncorrect)?;
        if !challenge.confirm.is_empty() {
            let expected = key_confirm(&shared_key);
            if challenge.confirm != expected {
                return Err(PairError::PinIncorrect);
            }
        }
        Ok(PinOutcome {
            fingerprint: challenge.fingerprint,
            shared_key,
        })
    }
}

/// 用共享密钥生成密钥确认值（HMAC-SHA256）。
pub fn key_confirm(shared_key: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(shared_key).expect("HMAC 接受任意长度密钥");
    mac.update(CONFIRM_LABEL);
    mac.finalize().into_bytes().to_vec()
}

/// 构造客户端首条挑战载荷。
pub fn client_challenge(spake2: &[u8], fingerprint: Fingerprint) -> Vec<u8> {
    postcard::to_allocvec(&PairChallenge {
        spake2: spake2.to_vec(),
        fingerprint,
        confirm: Vec::new(),
    })
    .expect("配对载荷编码不应失败")
}

/// 构造主机应答挑战载荷（含密钥确认值）。
pub fn host_challenge(spake2: &[u8], fingerprint: Fingerprint, shared_key: &[u8]) -> Vec<u8> {
    postcard::to_allocvec(&PairChallenge {
        spake2: spake2.to_vec(),
        fingerprint,
        confirm: key_confirm(shared_key),
    })
    .expect("配对载荷编码不应失败")
}

/// 校验 PIN 格式（长度必须等于 `pin_len` 且全为数字）。
pub fn validate_pin(pin: &str, policy: &PairingPolicy) -> Result<(), PairError> {
    if pin.len() != policy.pin_len || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(PairError::PinIncorrect);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_defaults_match_doc() {
        let p = PairingPolicy::default();
        assert_eq!(p.pin_len, 4);
        assert_eq!(p.pin_ttl, Duration::from_secs(120));
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.lockout, Duration::from_secs(60));
    }

    #[test]
    fn spake2_round_trip_with_same_pin() {
        let fp_a = Fingerprint::from_bytes([1u8; 32]);
        let fp_b = Fingerprint::from_bytes([2u8; 32]);

        let (client, msg_a) = PinPairing::start("1234");
        let challenge_a = client_challenge(&msg_a, fp_a);

        let (host_outcome, msg_b) =
            PinPairing::respond(&challenge_a, "1234").expect("PIN 正确应成功");
        let challenge_b = host_challenge(&msg_b, fp_b, &host_outcome.shared_key);

        let client_outcome = client.finish(&challenge_b).expect("密钥确认应通过");
        assert_eq!(client_outcome.fingerprint, fp_b);
        assert_eq!(host_outcome.fingerprint, fp_a);
        assert_eq!(client_outcome.shared_key, host_outcome.shared_key);
    }

    #[test]
    fn wrong_pin_is_detected() {
        let fp_a = Fingerprint::from_bytes([1u8; 32]);
        let fp_b = Fingerprint::from_bytes([2u8; 32]);

        let (client, msg_a) = PinPairing::start("1234");
        let challenge_a = client_challenge(&msg_a, fp_a);

        // 主机用了错误的 PIN，两边派生出的密钥不同。
        let (host_outcome, msg_b) =
            PinPairing::respond(&challenge_a, "9999").expect("SPAKE2 不报错");
        let challenge_b = host_challenge(&msg_b, fp_b, &host_outcome.shared_key);

        let err = client.finish(&challenge_b).expect_err("PIN 不同必须失败");
        assert_eq!(err.code(), 3001);
    }

    #[test]
    fn attempt_guard_locks_after_max_attempts() {
        let policy = PairingPolicy::default();
        let mut guard = AttemptGuard::new(policy);
        let now = Instant::now();
        assert_eq!(guard.remaining_attempts(), 3);
        guard.record_failure(now).expect("第一次失败不锁定");
        guard.record_failure(now).expect("第二次失败不锁定");
        let err = guard.record_failure(now).expect_err("第三次必须锁定");
        assert_eq!(err.code(), 3002);
        assert!(guard.is_locked(now));
        assert!(guard.check(now).is_err());
    }

    #[test]
    fn token_expires() {
        let token = PairingToken::issue(Duration::from_secs(120));
        assert!(token.verify(Instant::now()).is_ok());
        let err = token
            .verify(Instant::now() + Duration::from_secs(121))
            .expect_err("过期必须失败");
        assert_eq!(err.code(), 3003);
    }

    #[test]
    fn pin_validation() {
        let policy = PairingPolicy::default();
        assert!(validate_pin("1234", &policy).is_ok());
        assert!(validate_pin("123", &policy).is_err());
        assert!(validate_pin("abcd", &policy).is_err());
    }
}
