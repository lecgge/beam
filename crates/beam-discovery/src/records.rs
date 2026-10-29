//! 广播 / 浏览信息模型（§3.4）。
//!
//! mDNS 服务类型：`_beam._udp.local`
//! TXT 字段（§3.4）：`id`、`name`、`ver`、`fp`（指纹前 8 位）、`port`。

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// mDNS 服务类型（§3.4）。
pub const SERVICE_TYPE: &str = "_beam._udp.local";

/// TXT 字段名。
pub mod txt {
    /// 设备 id。
    pub const ID: &str = "id";
    /// 设备名。
    pub const NAME: &str = "name";
    /// 协议 / 软件版本。
    pub const VERSION: &str = "ver";
    /// 证书指纹前 8 位。
    pub const FINGERPRINT: &str = "fp";
    /// 服务端口。
    pub const PORT: &str = "port";
}

/// 本机广播信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    /// 设备 id（稳定不变）。
    pub id: String,
    /// 设备名（展示用）。
    pub name: String,
    /// 版本串。
    pub version: String,
    /// 证书指纹（十六进制；广播时只取前 8 位）。
    pub fingerprint: String,
    /// 服务端口。
    pub port: u16,
    /// 广播地址。
    pub address: Ipv4Addr,
}

impl HostInfo {
    /// 生成 TXT 字段（§3.4）。
    pub fn to_txt(&self) -> Vec<(String, String)> {
        vec![
            (txt::ID.to_string(), self.id.clone()),
            (txt::NAME.to_string(), self.name.clone()),
            (txt::VERSION.to_string(), self.version.clone()),
            // 指纹只广播前 8 位。
            (
                txt::FINGERPRINT.to_string(),
                self.fingerprint.chars().take(8).collect(),
            ),
            (txt::PORT.to_string(), self.port.to_string()),
        ]
    }
}

/// 发现到的主机。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredHost {
    /// 设备 id。
    pub id: String,
    /// 设备名。
    pub name: String,
    /// 版本串。
    pub version: String,
    /// 指纹前 8 位。
    pub fingerprint: String,
    /// 服务端口。
    pub port: u16,
    /// 地址。
    pub address: Ipv4Addr,
    /// mDNS 全名。
    pub fullname: String,
}

/// 从 TXT 字段还原主机信息。
pub fn host_from_txt(
    fullname: &str,
    address: Ipv4Addr,
    port: u16,
    txt: &[(String, String)],
) -> DiscoveredHost {
    let get = |key: &str| {
        txt.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let txt_port = get(txt::PORT).parse::<u16>().unwrap_or(port);
    DiscoveredHost {
        id: get(txt::ID),
        name: get(txt::NAME),
        version: get(txt::VERSION),
        fingerprint: get(txt::FINGERPRINT),
        port: txt_port,
        address,
        fullname: fullname.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_contains_required_fields() {
        let info = HostInfo {
            id: "abc".into(),
            name: "客厅电脑".into(),
            version: "0.1.0".into(),
            fingerprint: "0123456789abcdef".into(),
            port: 4827,
            address: Ipv4Addr::new(192, 168, 1, 10),
        };
        let fields = info.to_txt();
        let get = |k: &str| {
            fields
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get(txt::ID), "abc");
        assert_eq!(get(txt::NAME), "客厅电脑");
        assert_eq!(get(txt::VERSION), "0.1.0");
        assert_eq!(get(txt::PORT), "4827");
    }

    #[test]
    fn fingerprint_is_truncated_to_eight_chars() {
        let info = HostInfo {
            id: "a".into(),
            name: "b".into(),
            version: "c".into(),
            fingerprint: "0123456789abcdef".into(),
            port: 1,
            address: Ipv4Addr::LOCALHOST,
        };
        let fields = info.to_txt();
        let fp = fields
            .iter()
            .find(|(k, _)| k == txt::FINGERPRINT)
            .map(|(_, v)| v.clone())
            .unwrap();
        assert_eq!(fp, "01234567");
    }

    #[test]
    fn host_can_be_rebuilt_from_txt() {
        let info = HostInfo {
            id: "abc".into(),
            name: "n".into(),
            version: "v".into(),
            fingerprint: "0123456789abcdef".into(),
            port: 4827,
            address: Ipv4Addr::new(10, 0, 0, 5),
        };
        let host = host_from_txt("n._beam._udp.local.", info.address, 4827, &info.to_txt());
        assert_eq!(host.id, "abc");
        assert_eq!(host.port, 4827);
        assert_eq!(host.fingerprint, "01234567");
    }
}
