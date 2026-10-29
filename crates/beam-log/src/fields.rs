//! 日志字段规范（§3.15：`session_id`、`peer`、`stage`、`code`）。
//!
//! UI 与排障都靠这几个固定字段串联日志，因此统一在这里构造。

/// 标准日志字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFields {
    /// 会话号。
    pub session_id: Option<u64>,
    /// 对端标识（主机名 / 设备名 / 指纹前 8 位）。
    pub peer: Option<String>,
    /// 阶段（`capture` / `encode` / `net` / `decode` / `render` / `pair` / `session`）。
    pub stage: Option<String>,
    /// 错误码（`E` 段编号）。
    pub code: Option<u16>,
}

impl LogFields {
    /// 空字段集。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置会话号。
    pub fn with_session_id(mut self, v: u64) -> Self {
        self.session_id = Some(v);
        self
    }

    /// 设置对端标识。
    pub fn with_peer(mut self, v: impl Into<String>) -> Self {
        self.peer = Some(v.into());
        self
    }

    /// 设置阶段。
    pub fn with_stage(mut self, v: impl Into<String>) -> Self {
        self.stage = Some(v.into());
        self
    }

    /// 设置错误码。
    pub fn with_code(mut self, v: u16) -> Self {
        self.code = Some(v);
        self
    }

    /// 转成可直接展开进日志的键值对。
    pub fn to_pairs(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(v) = self.session_id {
            out.push(("session_id", v.to_string()));
        }
        if let Some(v) = &self.peer {
            out.push(("peer", v.clone()));
        }
        if let Some(v) = &self.stage {
            out.push(("stage", v.clone()));
        }
        if let Some(v) = self.code {
            out.push(("code", format!("E{v}")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_render_expected_keys() {
        let f = LogFields::new()
            .with_session_id(7)
            .with_peer("desktop")
            .with_stage("encode")
            .with_code(6001);
        let pairs = f.to_pairs();
        assert!(pairs.contains(&("session_id", "7".to_string())));
        assert!(pairs.contains(&("peer", "desktop".to_string())));
        assert!(pairs.contains(&("stage", "encode".to_string())));
        assert!(pairs.contains(&("code", "E6001".to_string())));
    }

    #[test]
    fn unset_fields_are_omitted() {
        let f = LogFields::new().with_stage("net");
        let pairs = f.to_pairs();
        assert_eq!(pairs.len(), 1);
    }
}
