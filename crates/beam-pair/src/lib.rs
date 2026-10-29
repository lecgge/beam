//! # beam-pair —— Beam 配对层
//!
//! 对应技术设计文档 **§3.3**（模块设计）与 **§5**（协议规格）。
//!
//! ## 职责
//!
//! Ed25519 自签证书（rcgen）、信任库、二维码载荷 `beam://pair?h=ip:port&fp=..&t=token`、
//! SPAKE2 PIN 配对、实现 rustls 校验器。
//!
//! ## 边界
//!
//! - **负责**：身份材料、指纹固定、信任库、SPAKE2 PIN、二维码载荷格式。
//! - **不负责**：网络 IO（PIN 交换消息由 session 经 net 传递，§3.3）。
//! - **依赖**：proto、rustls、rcgen、spake2。
//! - **被依赖**：net、session、ffi。
//! - **禁止**：依赖 UI、依赖任何平台 crate。
//!
//! ## 接口
//!
//! - [`Identity::load_or_create`]：载入或生成自签身份。
//! - [`Identity::qr_payload`]：生成二维码载荷。
//! - [`PinPairing::start`] / [`PinPairing::respond`] / [`PinPairing::finish`]：SPAKE2 PIN 流程。
//! - [`TrustStore`]：信任库，实现 [`PeerVerifier`]。
//! - [`rustls_verifier`]：把信任库接到 rustls 握手校验上。
//!
//! ## 数据结构
//!
//! [`Fingerprint`]（`[u8;32]`）、[`TrustedPeer`]（`fp/name/platform/paired_at/perms`）。
//!
//! ## 线程模型
//!
//! 同步 API，在调用方任务内执行（§3.3）。
//!
//! ## 错误码
//!
//! `E3xxx`（§3.3）：`3001` PIN 错误、`3002` 次数超限锁定 60 s、`3003` token 过期、
//! `3004` 指纹不匹配；另有同段扩展 `3005`–`3007`（协议 / 存储 / 载荷）。
//!
//! ## 配置
//!
//! `pin_len=4`、`pin_ttl=120s`、`max_attempts=3`（[`PairingPolicy`]）。
//!
//! ## 性能
//!
//! 不在热路径。
//!
//! ## 测试
//!
//! 中间人模拟、错误 PIN、过期 token、SPAKE2 双方一致性。

#![forbid(unsafe_code)]

pub mod error;
pub mod fingerprint;
pub mod identity;
pub mod pin;
pub mod qr;
pub mod trust;
pub mod verifier;

pub use error::{message, PairError};
pub use fingerprint::{Fingerprint, PeerVerifier};
pub use identity::{
    default_identity_path, generate_token, FileKeyStore, Identity, KeyStore, MemoryKeyStore,
};
pub use pin::{
    client_challenge, host_challenge, key_confirm, validate_pin, AttemptGuard, PairChallenge,
    PairingPolicy, PairingToken, PinOutcome, PinPairing,
};
pub use qr::{PairPayload, URI_PREFIX};
pub use trust::{PeerPerms, TrustStore, TrustedPeer};
pub use verifier::{rustls_verifier, PeerCertVerifier};

/// ALPN 标识（§5），与 [`beam_proto::ALPN`] 保持一致。
pub const ALPN: &[u8] = beam_proto::ALPN;
