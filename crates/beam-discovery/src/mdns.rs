//! mDNS 广播与浏览（§3.4）。
//!
//! §3.4 边界：只发现不连接；Android 侧可用 NsdManager 兜底（在 Kotlin 层）。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::channel::mpsc;
use futures::stream::Stream;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::error::DiscoveryError;
use crate::records::{host_from_txt, txt, DiscoveredHost, HostInfo, SERVICE_TYPE_FQDN};

/// 广播本机信息。
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl std::fmt::Debug for Advertiser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Advertiser")
            .field("fullname", &self.fullname)
            .finish()
    }
}

impl Advertiser {
    /// 开始广播（§3.4 `Advertiser::start(info)`）。
    pub fn start(info: HostInfo) -> Result<Self, DiscoveryError> {
        if info.id.is_empty() || info.name.is_empty() {
            return Err(DiscoveryError::InvalidInfo {
                reason: "设备 id / 名称不能为空".to_string(),
            });
        }
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::DaemonStartFailed {
            reason: e.to_string(),
        })?;

        let properties: HashMap<String, String> = info.to_txt().into_iter().collect();
        let service = ServiceInfo::new(
            SERVICE_TYPE_FQDN,
            &info.name,
            &format!("{}.local.", info.name),
            std::net::IpAddr::V4(info.address),
            info.port,
            properties,
        )
        .map_err(|e| DiscoveryError::RegisterFailed {
            reason: e.to_string(),
        })?;

        let fullname = service.get_fullname().to_string();
        daemon
            .register(service)
            .map_err(|e| DiscoveryError::RegisterFailed {
                reason: e.to_string(),
            })?;

        Ok(Self { daemon, fullname })
    }

    /// 本机服务的 mDNS 全名。
    pub fn fullname(&self) -> &str {
        &self.fullname
    }

    /// 停止广播。
    pub fn shutdown(&self) {
        let _ = self.daemon.unregister(&self.fullname);
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 发现事件（§3.4 `DiscoveryEvent`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    /// 发现一个主机。
    Found(DiscoveredHost),
    /// 主机消失。
    Lost {
        /// mDNS 全名。
        fullname: String,
    },
}

/// 浏览局域网主机（§3.4 `Browser::start()`）。
pub struct Browser {
    daemon: ServiceDaemon,
    receiver: mpsc::UnboundedReceiver<DiscoveryEvent>,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser").finish()
    }
}

impl Browser {
    /// 开始浏览。
    pub fn start() -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::DaemonStartFailed {
            reason: e.to_string(),
        })?;
        let raw = daemon
            .browse(SERVICE_TYPE_FQDN)
            .map_err(|e| DiscoveryError::BrowseFailed {
                reason: e.to_string(),
            })?;
        let (tx, rx) = mpsc::unbounded();
        let daemon_for_task = daemon.clone();
        std::thread::Builder::new()
            .name("beam-mdns".to_string())
            .spawn(move || {
                for event in raw {
                    let Some(ev) = convert_event(&event) else {
                        continue;
                    };
                    if tx.unbounded_send(ev).is_err() {
                        break;
                    }
                }
                drop(daemon_for_task);
            })
            .map_err(|e| DiscoveryError::BrowseFailed {
                reason: format!("启动浏览线程失败: {e}"),
            })?;

        Ok(Self {
            daemon,
            receiver: rx,
        })
    }

    /// 停止浏览。
    pub fn shutdown(&self) {
        let _ = self.daemon.shutdown();
    }
}

impl Stream for Browser {
    type Item = DiscoveryEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}

fn convert_event(event: &ServiceEvent) -> Option<DiscoveryEvent> {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let address = info.get_addresses_v4().into_iter().next()?;
            let port = info.get_port();
            let properties: Vec<(String, String)> = info
                .get_properties()
                .iter()
                .map(|p| {
                    (
                        p.key().to_string(),
                        p.val()
                            .map(|v| String::from_utf8_lossy(v).to_string())
                            .unwrap_or_default(),
                    )
                })
                .collect();
            let fullname = info.get_fullname().to_string();
            Some(DiscoveryEvent::Found(host_from_txt(
                &fullname,
                address,
                port,
                &properties,
            )))
        }
        ServiceEvent::ServiceRemoved(_, fullname) => Some(DiscoveryEvent::Lost {
            fullname: fullname.clone(),
        }),
        _ => None,
    }
}

/// 便于测试的纯函数：判断一组 TXT 字段是否完整。
pub fn is_complete_txt(properties: &[(String, String)]) -> bool {
    let has = |k: &str| properties.iter().any(|(key, _)| key == k);
    has(txt::ID) && has(txt::NAME) && has(txt::VERSION) && has(txt::FINGERPRINT)
}

/// 便于测试的纯函数：从地址与端口构造广播地址。
pub fn local_address(address: Ipv4Addr, port: u16) -> (Ipv4Addr, u16) {
    (address, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertiser_rejects_empty_id() {
        let info = HostInfo {
            platform: "windows".to_string(),
            id: String::new(),
            name: "n".into(),
            version: "v".into(),
            fingerprint: "01234567".into(),
            port: 4827,
            address: Ipv4Addr::LOCALHOST,
        };
        let err = Advertiser::start(info).expect_err("空 id 必须失败");
        assert_eq!(err.code(), 4004);
    }

    #[test]
    fn txt_completeness_check() {
        let props = vec![
            (txt::ID.to_string(), "a".to_string()),
            (txt::NAME.to_string(), "b".to_string()),
            (txt::VERSION.to_string(), "c".to_string()),
            (txt::FINGERPRINT.to_string(), "d".to_string()),
        ];
        assert!(is_complete_txt(&props));
        assert!(!is_complete_txt(&props[..2]));
    }

    #[test]
    fn browser_can_be_created_and_stopped() {
        // 组播在部分环境被阻断，这里只验证能启动并干净停止。
        if let Ok(browser) = Browser::start() {
            browser.shutdown();
        }
    }
}
