//! PIN 校验与尝试预算(§3.3)。
//!
//! ## 为什么要有这个模块
//!
//! 早先 `connect` command **根本没有 PIN 参数**(§3.11 的清单是 `connect(host_id)`),
//! 于是前端那个 4 位 PIN 输入框是**纯装饰**:填对填错都能连上。
//!
//! S11 裁决:command 补 PIN 参数 + **Rust 侧**做格式校验与尝试预算;
//! 主机侧的 SPAKE2 挑战留待 §3.3(主机侧还没把 PIN 配进 `PairingConfig`)。
//!
//! ## 边界(诚实说明)
//!
//! 这一层做的是:
//! - **格式校验**:必须是 4 位数字(§3.3 `pin_len=4`),否则 `E3001`;
//! - **尝试预算**:连续失败超过 3 次即锁定 60 秒(§3.3 `max_attempts=3`),报 `E3002`。
//!
//! 它**不**做密码学验证:PIN 是否与主机一致由主机侧的 SPAKE2 交换决定(§3.3),
//! 那一段尚未接线。因此本模块只保证「PIN 参与连接流程且被计量」,
//! 不会让 UI 假装已经完成密码学认证。
//!
//! 错误码取自 `beam-pair`(§3.3 `E3xxx`):`E3001` PIN 错误 / `E3002` 次数超限。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::error::{BeamError, E_PIN_INVALID, E_PIN_LOCKED};

/// §3.3 `pin_len=4`。
pub const PIN_LENGTH: usize = 4;

/// §3.3 `max_attempts=3`。
pub const MAX_ATTEMPTS: u32 = 3;

/// §3.3 `E3002`:锁定 60 秒。
pub const LOCKOUT: Duration = Duration::from_secs(60);

/// 校验 PIN 格式(§3.3 `pin_len=4`):必须正好 4 位 ASCII 数字。
///
/// 返回 `Err` 时给出**中文原因**,直接进 `BeamError.message`。
pub fn validate_format(pin: &str) -> Result<(), String> {
    if pin.len() != PIN_LENGTH {
        return Err(format!(
            "PIN 必须是 {PIN_LENGTH} 位数字,当前是 {} 位",
            pin.chars().count()
        ));
    }
    if !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err("PIN 只能包含数字".to_string());
    }
    Ok(())
}

/// 每台主机的 PIN 尝试预算(§3.3 `max_attempts=3` / `E3002` 锁定 60 s)。
///
/// 按主机 id 分开记账:对 A 主机的错误 PIN 不该锁死 B 主机的连接。
#[derive(Debug, Default)]
pub struct AttemptBudget {
    /// 每台主机的失败次数。
    failures: HashMap<String, u32>,
    /// 每台主机的锁定到期时刻。
    locked_until: HashMap<String, Instant>,
}

impl AttemptBudget {
    /// 新建一个空预算。
    pub fn new() -> Self {
        Self::default()
    }

    /// 该主机当前是否处于锁定期。
    pub fn is_locked(&self, host_id: &str, now: Instant) -> bool {
        matches!(self.locked_until.get(host_id), Some(until) if now < *until)
    }

    /// 该主机的剩余尝试次数。
    pub fn remaining(&self, host_id: &str) -> u32 {
        MAX_ATTEMPTS.saturating_sub(self.failures.get(host_id).copied().unwrap_or(0))
    }

    /// 连接前检查:锁定中直接拒绝(`E3002`)。
    pub fn check(&self, host_id: &str, now: Instant) -> Result<(), String> {
        if self.is_locked(host_id, now) {
            return Err(format!(
                "PIN 尝试次数超过上限 {MAX_ATTEMPTS},已锁定 {} 秒",
                LOCKOUT.as_secs()
            ));
        }
        Ok(())
    }

    /// 记一次失败;达到上限即锁定 60 秒并返回 `Err`(`E3002`)。
    pub fn record_failure(&mut self, host_id: &str, now: Instant) -> Result<(), String> {
        let count = self.failures.entry(host_id.to_string()).or_insert(0);
        *count += 1;
        if *count >= MAX_ATTEMPTS {
            self.locked_until.insert(host_id.to_string(), now + LOCKOUT);
            return Err(format!(
                "PIN 尝试次数超过上限 {MAX_ATTEMPTS},已锁定 {} 秒",
                LOCKOUT.as_secs()
            ));
        }
        Ok(())
    }

    /// 连接成功后复位该主机的预算。
    pub fn reset(&mut self, host_id: &str) {
        self.failures.remove(host_id);
        self.locked_until.remove(host_id);
    }
}

/// 连接前的 PIN 准入检查(§3.3)。
///
/// - **已信任的主机**:信任库里已有它的指纹(§3.3「证书指纹固定」),
///   不需要再输 PIN ⇒ 传空串即可。
/// - **未信任的主机**:必须给 PIN,且格式合法(4 位数字)。
/// - **预算锁定中**:直接拒绝(`E3002`)。
///
/// 返回 `Err` 时**不**消耗预算 —— 锁定检查失败不是一次「尝试」。
/// 格式错误由调用方决定是否计入(见 [`AttemptBudget::record_failure`] 的使用点)。
pub fn check_connect_pin(
    budget: &AttemptBudget,
    host_id: &str,
    trusted: bool,
    pin: &str,
    now: Instant,
) -> Result<(), BeamError> {
    // 预算检查在最前:锁定期间连格式都不必看。
    if let Err(reason) = budget.check(host_id, now) {
        return Err(BeamError::invalid(E_PIN_LOCKED, reason));
    }
    if trusted {
        // 已信任的主机不需要 PIN;给了也只做格式校验(不参与任何校验语义)。
        if pin.is_empty() {
            return Ok(());
        }
    }
    match validate_format(pin) {
        Ok(()) => Ok(()),
        Err(reason) => Err(BeamError::invalid(E_PIN_INVALID, reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_accepts_four_digits() {
        assert!(validate_format("0000").is_ok());
        assert!(validate_format("4827").is_ok());
    }

    #[test]
    fn format_rejects_wrong_length_and_non_digits() {
        assert!(validate_format("").is_err());
        assert!(validate_format("123").is_err());
        assert!(validate_format("12345").is_err());
        assert!(validate_format("12a4").is_err());
        assert!(validate_format("１２３４").is_err(), "全角数字不算 ASCII 数字");
    }

    #[test]
    fn budget_locks_after_max_attempts() {
        let mut budget = AttemptBudget::new();
        let now = Instant::now();
        assert_eq!(budget.remaining("h1"), MAX_ATTEMPTS);
        assert!(budget.record_failure("h1", now).is_ok());
        assert!(budget.record_failure("h1", now).is_ok());
        assert_eq!(budget.remaining("h1"), 1);
        let err = budget.record_failure("h1", now).expect_err("第三次必须锁定");
        assert!(err.contains("锁定"));
        assert!(budget.is_locked("h1", now));
        assert!(budget.check("h1", now).is_err());
    }

    #[test]
    fn budget_is_per_host() {
        let mut budget = AttemptBudget::new();
        let now = Instant::now();
        for _ in 0..MAX_ATTEMPTS {
            let _ = budget.record_failure("h1", now);
        }
        assert!(budget.is_locked("h1", now));
        // 另一台主机不受影响。
        assert!(!budget.is_locked("h2", now));
        assert_eq!(budget.remaining("h2"), MAX_ATTEMPTS);
        assert!(budget.check("h2", now).is_ok());
    }

    #[test]
    fn budget_resets_after_success() {
        let mut budget = AttemptBudget::new();
        let now = Instant::now();
        let _ = budget.record_failure("h1", now);
        budget.reset("h1");
        assert_eq!(budget.remaining("h1"), MAX_ATTEMPTS);
        assert!(!budget.is_locked("h1", now));
    }

    /// 锁定到期后自动解锁(锁定期只持续 60 秒)。
    #[test]
    fn lockout_expires() {
        let mut budget = AttemptBudget::new();
        let now = Instant::now();
        for _ in 0..MAX_ATTEMPTS {
            let _ = budget.record_failure("h1", now);
        }
        assert!(budget.is_locked("h1", now));
        let later = now + LOCKOUT + Duration::from_secs(1);
        assert!(!budget.is_locked("h1", later));
        assert!(budget.check("h1", later).is_ok());
    }
}
