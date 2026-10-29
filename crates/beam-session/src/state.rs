//! 会话状态机（§3.9）。
//!
//! §3.9 状态：`Idle → Discovering → Pairing → Connecting → Negotiating → Streaming ⇄ Reconnecting → Closed`；
//! `Error` 可从任意状态进入。
//!
//! §4.1 时序：`Idle → Connecting → (Pairing) → Negotiating → Streaming`；
//! `Streaming → Reconnecting(3 s 无数据) → Streaming(30 s 内恢复) | Closed(超时)`；
//! `任意 → Error → Closed`。
//!
//! 两处描述合并后的转移表见 [`SessionState::allowed_next`]。

use std::fmt;

use crate::error::SessionError;

/// 会话状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SessionState {
    /// 空闲（初始态）。
    #[default]
    Idle,
    /// 正在发现主机。
    Discovering,
    /// 正在配对。
    Pairing,
    /// 正在连接。
    Connecting,
    /// 正在协商能力。
    Negotiating,
    /// 正在串流。
    Streaming,
    /// 断线重连中。
    Reconnecting,
    /// 已关闭（终态）。
    Closed,
    /// 错误（可转入 `Closed`）。
    Error,
}

/// 全部状态（表驱动测试用）。
pub const ALL_STATES: [SessionState; 9] = [
    SessionState::Idle,
    SessionState::Discovering,
    SessionState::Pairing,
    SessionState::Connecting,
    SessionState::Negotiating,
    SessionState::Streaming,
    SessionState::Reconnecting,
    SessionState::Closed,
    SessionState::Error,
];

impl SessionState {
    /// 该状态允许的后继状态。
    pub fn allowed_next(self) -> &'static [SessionState] {
        match self {
            // 任意非终态都可以进入 Error；Error 只能进入 Closed。
            SessionState::Idle => &[
                SessionState::Discovering,
                SessionState::Pairing,
                SessionState::Connecting,
                SessionState::Closed,
            ],
            SessionState::Discovering => &[
                SessionState::Pairing,
                SessionState::Connecting,
                SessionState::Closed,
            ],
            SessionState::Pairing => &[SessionState::Connecting, SessionState::Closed],
            SessionState::Connecting => &[SessionState::Negotiating, SessionState::Closed],
            SessionState::Negotiating => &[SessionState::Streaming, SessionState::Closed],
            SessionState::Streaming => &[SessionState::Reconnecting, SessionState::Closed],
            SessionState::Reconnecting => &[SessionState::Streaming, SessionState::Closed],
            SessionState::Closed => &[],
            SessionState::Error => &[SessionState::Closed],
        }
    }

    /// 是否允许转移到 `next`。
    ///
    /// 除转移表之外，还额外允许**任意非终态 → Error**（§3.9「Error 可从任意状态进入」）。
    pub fn can_transition_to(self, next: SessionState) -> bool {
        if self == SessionState::Closed {
            return false;
        }
        if next == SessionState::Error {
            // 「Error 可从任意状态进入」：终态除外，且 Error 自身不再重复进入。
            return self != SessionState::Error;
        }
        self.allowed_next().contains(&next)
    }

    /// 是否是终态。
    pub fn is_terminal(self) -> bool {
        self == SessionState::Closed
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SessionState::Idle => "Idle",
            SessionState::Discovering => "Discovering",
            SessionState::Pairing => "Pairing",
            SessionState::Connecting => "Connecting",
            SessionState::Negotiating => "Negotiating",
            SessionState::Streaming => "Streaming",
            SessionState::Reconnecting => "Reconnecting",
            SessionState::Closed => "Closed",
            SessionState::Error => "Error",
        };
        f.write_str(s)
    }
}

/// 会话状态机。
#[derive(Debug, Clone, Default)]
pub struct StateMachine {
    state: SessionState,
}

impl StateMachine {
    /// 新建状态机（初始态 `Idle`）。
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
        }
    }

    /// 当前状态。
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// 尝试转移；非法转移返回 `E9001`。
    pub fn transition(&mut self, next: SessionState) -> Result<SessionState, SessionError> {
        if !self.state.can_transition_to(next) {
            return Err(SessionError::IllegalTransition {
                from: self.state.to_string(),
                to: next.to_string(),
            });
        }
        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表驱动的合法转移表（§3.9 + §4.1）。
    const LEGAL: &[(SessionState, &[SessionState])] = &[
        (
            SessionState::Idle,
            &[
                SessionState::Discovering,
                SessionState::Pairing,
                SessionState::Connecting,
                SessionState::Closed,
                SessionState::Error,
            ],
        ),
        (
            SessionState::Discovering,
            &[
                SessionState::Pairing,
                SessionState::Connecting,
                SessionState::Closed,
                SessionState::Error,
            ],
        ),
        (
            SessionState::Pairing,
            &[
                SessionState::Connecting,
                SessionState::Closed,
                SessionState::Error,
            ],
        ),
        (
            SessionState::Connecting,
            &[
                SessionState::Negotiating,
                SessionState::Closed,
                SessionState::Error,
            ],
        ),
        (
            SessionState::Negotiating,
            &[
                SessionState::Streaming,
                SessionState::Closed,
                SessionState::Error,
            ],
        ),
        (
            SessionState::Streaming,
            &[
                SessionState::Reconnecting,
                SessionState::Closed,
                SessionState::Error,
            ],
        ),
        (
            SessionState::Reconnecting,
            &[
                SessionState::Streaming,
                SessionState::Closed,
                SessionState::Error,
            ],
        ),
        (SessionState::Error, &[SessionState::Closed]),
        (SessionState::Closed, &[]),
    ];

    #[test]
    fn table_driven_transitions_match_spec() {
        for from in ALL_STATES {
            for to in ALL_STATES {
                let expected = LEGAL
                    .iter()
                    .find(|(s, _)| *s == from)
                    .map(|(_, next)| next.contains(&to))
                    .unwrap_or(false);
                assert_eq!(
                    from.can_transition_to(to),
                    expected,
                    "转移 {from} -> {to} 与规格不符"
                );
            }
        }
    }

    #[test]
    fn closed_is_terminal() {
        assert!(SessionState::Closed.is_terminal());
        assert!(!SessionState::Closed.can_transition_to(SessionState::Streaming));
        assert!(!SessionState::Closed.can_transition_to(SessionState::Error));
    }

    #[test]
    fn error_can_be_entered_from_any_non_terminal_state() {
        for s in ALL_STATES {
            if s == SessionState::Closed || s == SessionState::Error {
                continue;
            }
            assert!(
                s.can_transition_to(SessionState::Error),
                "{s} 应当可以进入 Error"
            );
        }
        assert!(!SessionState::Error.can_transition_to(SessionState::Error));
    }

    #[test]
    fn machine_rejects_illegal_transition() {
        let mut m = StateMachine::new();
        assert_eq!(m.state(), SessionState::Idle);
        m.transition(SessionState::Connecting).expect("应当允许");
        assert_eq!(m.state(), SessionState::Connecting);
        let err = m
            .transition(SessionState::Streaming)
            .expect_err("Connecting 不能直接到 Streaming");
        assert_eq!(err.code(), 9001);
        assert_eq!(m.state(), SessionState::Connecting, "失败后状态不变");
    }

    #[test]
    fn full_mainline_path() {
        let mut m = StateMachine::new();
        for s in [
            SessionState::Discovering,
            SessionState::Pairing,
            SessionState::Connecting,
            SessionState::Negotiating,
            SessionState::Streaming,
            SessionState::Reconnecting,
            SessionState::Streaming,
            SessionState::Closed,
        ] {
            m.transition(s).expect("主链路必须合法");
        }
        assert_eq!(m.state(), SessionState::Closed);
    }
}
