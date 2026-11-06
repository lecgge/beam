//! 异步建链的公共助手（§4.1「新连接/迁移」）。
//!
//! 会话层是**同步**的，而建 QUIC 连接是**异步**的。
//! 本模块给出两侧的桥：
//!
//! - 上层（`beam-client` / `beam-ffi`）用 [`spawn_transport`] 在 tokio 运行时里
//!   **异步**建链；
//! - 会话层用 [`PendingTransport::poll_result`] **非阻塞**地取结果。
//!
//! 为什么不在会话层里直接 `block_on`：会话的 `pump()` 往往跑在
//! tokio 工作线程上（桌面端就是这样），`block_on` 会卡住运行时——
//! 而 QUIC 的 IO 驱动正在那个运行时里。

use std::future::Future;

use tokio::sync::oneshot;

use crate::error::SessionError;
use crate::event::{PendingTransport, Transport};

/// 用 tokio 的 oneshot 通道取回建链结果（**非阻塞**）。
struct ChannelPending {
    rx: oneshot::Receiver<Result<Box<dyn Transport>, SessionError>>,
}

impl PendingTransport for ChannelPending {
    fn poll_result(&mut self) -> Option<Result<Box<dyn Transport>, SessionError>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            // 还在建：会话层下一拍再问。
            Err(oneshot::error::TryRecvError::Empty) => None,
            // 建链任务被丢弃（例如运行时被关闭）：
            // 如实报错，不能当成「还在建」。
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(SessionError::TransportFailed {
                reason: "建链任务已结束但没有给出结果(运行时可能已关闭)".to_string(),
            })),
        }
    }
}

/// 在 tokio 运行时里**异步**建一条传输（§4.1「新连接/迁移」）。
///
/// - `handle`：建链用的运行时（QUIC 的 IO 驱动在它的工作线程上）；
/// - `fut`：建链的异步过程（必须 `'static + Send`）。
///
/// 返回的句柄**不阻塞**：会话层在 `pump` 里用
/// [`PendingTransport::poll_result`] 取结果。
pub fn spawn_transport<F>(handle: &tokio::runtime::Handle, fut: F) -> Box<dyn PendingTransport>
where
    F: Future<Output = Result<Box<dyn Transport>, SessionError>> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    handle.spawn(async move {
        let _ = tx.send(fut.await);
    });
    Box::new(ChannelPending { rx })
}

/// 取当前 tokio 运行时（没有运行时返回 `None`）。
///
/// 上层若已知自己在 tokio 里（例如桌面端的 `async` 任务里），
/// 可以直接用它；若在**非**运行时线程上（例如 Android 的 FFI 调用方），
/// 应该把自己的 `Handle` 传给工厂。
pub fn current_handle() -> Option<tokio::runtime::Handle> {
    tokio::runtime::Handle::try_current().ok()
}
