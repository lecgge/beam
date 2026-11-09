//! 客户端的**信任引导**边界（§3.3「证书指纹固定」）。
//!
//! ## 它钉住什么
//!
//! 客户端的 TLS 校验器是**信任库**（[`beam_pair::TrustStore`]）—— 只接受库里
//! 已经在册的对端指纹。§3.4 的 mDNS 只广播指纹**前 8 位**，所以「发现到一台
//! 主机」**不等于**「能连上它」：
//!
//! - 信任库里有它的完整指纹 ⇒ 握手通过（证书固定生效）；
//! - 信任库里没有 ⇒ 握手在 TLS 层就被拒（`E2002`），控制通道根本开不了，
//!   PIN / SPAKE2 也送不过去。
//!
//! 这条边界是**故意的**（§3.3：证书指纹固定 ⇒ 防中间人），但它同时说明：
//! 客户端第一次连一台新主机时，**必须先有办法把它的完整指纹写进信任库**
//! （扫码载荷带完整指纹，或探测完整指纹后由用户确认）。
//!
//! 本测试把这条边界钉成可执行的断言，免得以后有人误以为「发现了就能连」。

use std::sync::Arc;

use beam_net::{NetConfig, NetEndpoint, TlsIdentity};
use beam_pair::{Identity, TrustStore, TrustedPeer};

/// 主机侧网络参数（自签证书 + 回环监听）。
fn host_config(identity: &Identity) -> NetConfig {
    NetConfig {
        identity: Some(TlsIdentity {
            cert_chain: identity.certificate_chain(),
            key: identity.private_key(),
        }),
        listen_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
        bind_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
        server_name: "beam.local".to_string(),
        ..NetConfig::default()
    }
}

/// 客户端侧网络参数。
fn client_config(identity: &Identity) -> NetConfig {
    NetConfig {
        identity: Some(TlsIdentity {
            cert_chain: identity.certificate_chain(),
            key: identity.private_key(),
        }),
        listen_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
        bind_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
        server_name: "beam.local".to_string(),
        ..NetConfig::default()
    }
}

#[tokio::test]
async fn client_cannot_tls_connect_to_an_untrusted_host() {
    let host_identity = Identity::generate().expect("生成主机身份");
    let client_identity = Identity::generate().expect("生成客户端身份");

    // 主机侧信任**客户端**（客户端的证书没问题），这样被拒的只有主机那边。
    let mut host_trust = TrustStore::new();
    host_trust.upsert(TrustedPeer::new(
        client_identity.fingerprint(),
        "回环客户端",
        "windows",
        0,
    ));
    let listener = NetEndpoint::listen(host_config(&host_identity), Arc::new(host_trust))
        .await
        .expect("主机应当能监听");
    let addr = listener.local_addr().expect("读本机地址");

    // 主机必须**真的去接受**连接，握手才能走完（否则对端只会等到超时）。
    tokio::spawn(async move {
        // 失败的握手**不算数**：继续等下一条连接（桌面端的主机监听也是这么做的）。
        loop {
            match listener.accept().await {
                Ok(conn) => {
                    conn.close(0, b"bye");
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
    });

    // 客户端的信任库是**空的**（还没信任过这台主机）。
    let empty_trust = TrustStore::new();
    let err = NetEndpoint::connect(addr, client_config(&client_identity), Arc::new(empty_trust))
        .await
        .expect_err("未受信任的主机必须被 TLS 层拒绝");
    assert_eq!(
        err.code(),
        2002,
        "必须是「证书被拒」(E2002)，而不是别的错误：{err}"
    );

    // 信任库里有了它的完整指纹 ⇒ 同样的连接就通了（证书固定生效）。
    //
    // 等一下再试：第一次握手是被**客户端**拒掉的（证书不合规），
    // 主机侧要把那次失败的握手收尾，才好接下一次连接。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut trust = TrustStore::new();
    trust.upsert(TrustedPeer::new(
        host_identity.fingerprint(),
        "回环主机",
        "windows",
        0,
    ));
    let conn = NetEndpoint::connect(addr, client_config(&client_identity), Arc::new(trust))
        .await
        .expect("已信任的主机必须能连上");
    // 收尾：关掉连接，别把端口留着。
    conn.close(0, b"bye");
}

/// 对照组：客户端信任库里**有**主机指纹 ⇒ 握手通过。
#[tokio::test]
async fn client_connects_to_a_trusted_host() {
    let host_identity = Identity::generate().expect("生成主机身份");
    let client_identity = Identity::generate().expect("生成客户端身份");

    let mut host_trust = TrustStore::new();
    host_trust.upsert(TrustedPeer::new(
        client_identity.fingerprint(),
        "回环客户端",
        "windows",
        0,
    ));
    let listener = NetEndpoint::listen(host_config(&host_identity), Arc::new(host_trust))
        .await
        .expect("主机应当能监听");
    let addr = listener.local_addr().expect("读本机地址");

    tokio::spawn(async move {
        // 失败的握手**不算数**：继续等下一条连接（桌面端的主机监听也是这么做的）。
        loop {
            match listener.accept().await {
                Ok(conn) => {
                    conn.close(0, b"bye");
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
    });

    let mut trust = TrustStore::new();
    trust.upsert(TrustedPeer::new(
        host_identity.fingerprint(),
        "回环主机",
        "windows",
        0,
    ));
    let conn = NetEndpoint::connect(addr, client_config(&client_identity), Arc::new(trust))
        .await
        .expect("已信任的主机必须能连上");
    conn.close(0, b"bye");
}
