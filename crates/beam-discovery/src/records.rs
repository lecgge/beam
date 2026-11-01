//! 广播 / 浏览信息模型（§3.4）。
//!
//! mDNS 服务类型：`_beam._udp.local`
//! TXT 字段（§3.4）：`id`、`name`、`ver`、`fp`（指纹前 8 位）、`port`。

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// 对端没广播平台时的占位值（S1 裁决）。
pub const UNKNOWN_PLATFORM: &str = "未知平台";

/// mDNS 服务类型（§3.4）。
///
/// 这是**文档口径**的名字（§3.4 写的就是 `_beam._udp.local`）。
pub const SERVICE_TYPE: &str = "_beam._udp.local";

/// 同一服务类型的 **FQDN 形式**（结尾带点）。
///
/// `mdns-sd` 的 `register` / `browse` 都要求服务类型以
/// `._tcp.local.` 或 `._udp.local.` **结尾**（带那个点），
/// 否则直接返回 `RegisterFailed`：
/// 「mDNS service … must end with '._tcp.local.' or '._udp.local.'」。
///
/// 少了这个点 ⇒ mDNS 广播**根本注册不上去**，局域网里根本发现不了主机。
pub const SERVICE_TYPE_FQDN: &str = "_beam._udp.local.";

/// TXT 字段名。
pub mod txt {
    /// 设备 id。
    pub const ID: &str = "id";
    /// 设备名。
    pub const NAME: &str = "name";
    /// 协议 / 软件版本。
    pub const VERSION: &str = "ver";
    /// 指纹前 8 位。
    pub const FINGERPRINT: &str = "fp";
    /// 平台标识（追加字段，见 S1 裁决）。
    ///
    /// 广播自己的平台（`windows` / `android` …），
    /// 让信任库的 `TrustedPeer.platform` 不再恒为「未知平台」。
    /// 对端不带这个字段时按「未知平台」处理，不影响发现。
    pub const PLATFORM: &str = "plat";
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
    /// 本机平台标识（§3.4 TXT 的 `plat` 字段）。
    pub platform: String,
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
            (txt::PLATFORM.to_string(), self.platform.clone()),
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
    /// 对端平台（TXT 的 `plat` 字段；缺失时为「未知平台」）。
    pub platform: String,
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
        platform: match get(txt::PLATFORM) {
            v if v.is_empty() => UNKNOWN_PLATFORM.to_string(),
            v => v,
        },
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
            platform: "windows".to_string(),
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
        assert_eq!(get(txt::PLATFORM), "windows");
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
            platform: "windows".to_string(),
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
            platform: "windows".to_string(),
        };
        let host = host_from_txt("n._beam._udp.local.", info.address, 4827, &info.to_txt());
        assert_eq!(host.id, "abc");
        assert_eq!(host.port, 4827);
        assert_eq!(host.fingerprint, "01234567");
        assert_eq!(host.platform, "windows");
    }

    /// `mdns-sd` 的 `register` / `browse` 都要求服务类型以
    /// `._tcp.local.` / `._udp.local.` **结尾**（带那个点）。
    ///
    /// 少了这个点 ⇒ mDNS 广播**根本注册不上去**（`RegisterFailed`），
    /// 局域网里发现不了任何主机。
    #[test]
    fn service_type_fqdn_is_usable_by_mdns_sd() {
        assert!(SERVICE_TYPE_FQDN.ends_with("._udp.local."));
        assert_eq!(SERVICE_TYPE_FQDN, format!("{SERVICE_TYPE}."));
    }

    /// S1：`plat` 是**追加**字段 —— 对端（老版本）不带它时，
    /// 平台按「未知平台」处理，**发现本身不能受影响**。
    #[test]
    fn missing_platform_falls_back_to_unknown() {
        let txt = vec![
            (txt::ID.to_string(), "abc".to_string()),
            (txt::NAME.to_string(), "n".to_string()),
            (txt::VERSION.to_string(), "v".to_string()),
            (txt::FINGERPRINT.to_string(), "01234567".to_string()),
            (txt::PORT.to_string(), "4827".to_string()),
        ];
        let host = host_from_txt(
            "n._beam._udp.local.",
            Ipv4Addr::new(10, 0, 0, 5),
            4827,
            &txt,
        );
        assert_eq!(host.platform, UNKNOWN_PLATFORM);
        // 其余字段照常解析 —— 少一个字段不能让主机消失。
        assert_eq!(host.id, "abc");
        assert_eq!(host.port, 4827);
    }
}
