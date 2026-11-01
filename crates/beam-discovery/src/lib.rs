//! # beam-discovery —— Beam 局域网发现
//!
//! 对应技术设计文档 **§3.4**（模块设计）。
//!
//! ## 职责
//!
//! mDNS 广播 `_beam._udp.local`（TXT：`id`、`name`、`ver`、`fp` 前 8 位、`port`）与浏览。
//!
//! ## 边界
//!
//! - **负责**：广播本机信息、浏览并解析主机列表。
//! - **不负责**：只发现不连接（§3.4）。
//! - **依赖**：mdns-sd、proto。
//! - **被依赖**：host、client、ffi。
//!
//! ## 接口
//!
//! - [`Advertiser::start`]：开始广播。
//! - [`Browser::start`]：开始浏览，返回一个 `Stream<Item = DiscoveryEvent>`。
//!
//! ## 数据结构
//!
//! [`HostInfo`]（本机广播信息）、[`DiscoveredHost`]（发现到的主机）、
//! [`DiscoveryEvent`]（发现 / 消失事件）。
//!
//! ## 错误码
//!
//! `E4xxx`：`4001` mDNS 启动失败、`4002` 注册失败、`4003` 浏览失败、`4004` 广播信息非法。
//!
//! ## 配置
//!
//! `enabled`、`iface_allowlist`（由上层配置层传入）。
//!
//! ## 测试
//!
//! 多网卡、组播被阻断回退手动 IP（回退逻辑在 host / client 层）。
//! 本 crate 覆盖 TXT 字段生成与解析。

#![forbid(unsafe_code)]

pub mod error;
pub mod mdns;
pub mod records;

pub use error::{message, DiscoveryError};
pub use mdns::{Advertiser, Browser, DiscoveryEvent};
pub use records::{
    host_from_txt, DiscoveredHost, HostInfo, SERVICE_TYPE, SERVICE_TYPE_FQDN, UNKNOWN_PLATFORM,
};

/// TXT 字段名（§3.4）。
pub use records::txt;
