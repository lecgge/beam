//! 客户端真实上屏路径的验证（§3.10 / 产品方案第 45 行）。
//!
//! ## 这条测试验证什么
//!
//! 「独立原生窗口 + D3D11 flip-model 交换链 + D3D11VA 硬解」这条路径此前
//! **完全没有被构建过**（`beam-render/d3d11` 与 `beam-decode/ffmpeg` 都是
//! 默认关闭的 feature，`ClientRuntime` 里也没有任何建交换链的入口）。
//!
//! 本测试把整条路径**真的跑一遍**：
//!
//! 1. `beam-window` 建一个真实的顶层 Win32 窗口；
//! 2. `beam-render` 在这个窗口上建 D3D11 flip-model 交换链；
//! 3. `beam-decode` 用**同一个 D3D11 设备**打开 D3D11VA 硬解上下文
//!    （D3D11 不允许跨设备拷贝，所以必须是同一个）。
//!
//! 三步全过 ⇒ 交换链、设备、硬解后端在**本机**都是真的。
//!
//! ## 为什么默认 `#[ignore]`
//!
//! 需要 `video` feature（`beam-render/d3d11` + `beam-decode/ffmpeg` +
//! `beam-window/windows-api`）与 LGPL ffmpeg 开发库（§7），
//! CI 尚未引入 ffmpeg 发行版（见 `.github/workflows/ci.yml` 的说明），
//! 因此这条测试**不进 CI**，本地显式跑：
//!
//! ```powershell
//! $env:FFMPEG_DIR = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! cargo test -p beam-client --features video --test video_present -- --ignored
//! ```
//!
//! ## 它**没有**验证什么
//!
//! 本测试**不**产生任何视频帧，也不验证「画面真的出现在窗口里」——
//! 那需要一条真实的「主机抓屏 → 编码 → 网络 → 解码 → 上屏」链路，
//! 属于端到端验收（见台账的 ST-07 备注）。

#![cfg(all(windows, feature = "video"))]

use beam_client::{ClientConfig, ClientRuntime};
use beam_session::PeerAddr;

#[test]
#[ignore = "需要 `video` feature 与 LGPL ffmpeg；本地用 --ignored 显式跑"]
fn native_window_gets_a_real_swapchain_and_hardware_decoder() {
    let runtime = ClientRuntime::new(ClientConfig::default());
    let peer = PeerAddr {
        host: "127.0.0.1".into(),
        port: 4827,
        fingerprint: None,
    };

    // 建窗口 + 建交换链 + 开硬解上下文。
    let session = runtime
        .connect_to_window(peer, "Beam 上屏自检", 640, 480, None)
        .expect("原生窗口 + 交换链 + 硬解上下文应当都能建起来");

    assert!(
        session.window.hwnd() != 0,
        "窗口句柄不应为 0（0 表示没有窗口）"
    );
    // 没有传输 ⇒ 会话停在 Idle（**不会**伪造成已经在串流）。
    assert_eq!(session.handle.state(), beam_session::SessionState::Idle);
}
