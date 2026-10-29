//! rustls 校验器（§3.3「实现 rustls 校验器」、§4 `net → pair`）。
//!
//! 网络层把「对端证书是否可信」这件事完全委托给配对层：
//! 校验器只做三件事——算指纹、问信任库、验签名。

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};

use crate::fingerprint::{Fingerprint, PeerVerifier};

/// 基于 [`PeerVerifier`] 的 rustls 服务端证书校验器。
#[derive(Debug)]
pub struct PeerCertVerifier {
    inner: Arc<dyn PeerVerifier>,
}

impl PeerCertVerifier {
    /// 用信任库之类的校验器包装。
    pub fn new(inner: Arc<dyn PeerVerifier>) -> Self {
        Self { inner }
    }
}

impl ServerCertVerifier for PeerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let fp = Fingerprint::from_cert_der(end_entity.as_ref());
        if self.inner.verify(&fp) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(format!(
                "E3004: 设备指纹不匹配 ({})",
                fp.short_hex()
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        let algorithms = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, cert, dss, &algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        let algorithms = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, cert, dss, &algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// 构造一个 rustls 校验器（便捷函数）。
pub fn rustls_verifier(
    inner: Arc<dyn PeerVerifier>,
) -> Arc<dyn rustls::client::danger::ServerCertVerifier> {
    Arc::new(PeerCertVerifier::new(inner))
}

/// 双向认证用：主机端校验客户端证书（§产品方案 §8「TLS 1.3 双向认证」）。
#[derive(Debug)]
pub struct PeerClientCertVerifier {
    inner: Arc<dyn PeerVerifier>,
}

impl PeerClientCertVerifier {
    /// 用信任库之类的校验器包装。
    pub fn new(inner: Arc<dyn PeerVerifier>) -> Self {
        Self { inner }
    }
}

impl rustls::server::danger::ClientCertVerifier for PeerClientCertVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        // 自签证书场景没有 CA 根，客户端会直接发送自己的叶子证书。
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, TlsError> {
        let fp = Fingerprint::from_cert_der(end_entity.as_ref());
        if self.inner.verify(&fp) {
            Ok(rustls::server::danger::ClientCertVerified::assertion())
        } else {
            Err(TlsError::General(format!(
                "E3004: 设备指纹不匹配 ({})",
                fp.short_hex()
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        let algorithms = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, cert, dss, &algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        let algorithms = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, cert, dss, &algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// 构造主机端用的客户端证书校验器。
pub fn rustls_client_verifier(
    inner: Arc<dyn PeerVerifier>,
) -> Arc<dyn rustls::server::danger::ClientCertVerifier> {
    Arc::new(PeerClientCertVerifier::new(inner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::TrustStore;

    #[test]
    fn untrusted_certificate_is_rejected() {
        let store = TrustStore::new();
        let verifier = PeerCertVerifier::new(Arc::new(store));
        let cert = CertificateDer::from(vec![1u8, 2, 3]);
        let name = ServerName::try_from("beam.local").expect("应当是合法服务器名");
        let err = verifier
            .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
            .expect_err("未被信任的证书必须失败");
        assert!(err.to_string().contains("E3004"));
    }
}
