//! 客户端键鼠采集（§3.8「Windows 客户端由 beam-client 采集」）。
//!
//! ## 为什么是**低级钩子**
//!
//! - 普通窗口钩子（`WH_KEYBOARD` / `WH_MOUSE`）只能拿到**本进程**的输入；
//!   串流客户端要转发的是**全局**输入（用户在别的应用里操作）⇒ 必须用
//!   低级钩子（`WH_KEYBOARD_LL` / `WH_MOUSE_LL`）。
//! - 低级钩子的回调在**安装它的线程**上执行 ⇒ 该线程必须跑消息循环
//!   （`GetMessageW`），否则 Windows 会把钩子摘掉（钩子超时机制）。
//!
//! ## 键码口径（§3.8）
//!
//! 协议侧 [`beam_proto::InputEvent::Key`] 的 `code` 是 **USB HID 用法 ID**，
//! 而低级钩子给的是 Win32 **虚拟键码** ⇒ 用
//! [`beam_input::hid::vk_to_hid`] 翻译 —— 与主机侧注入用的 `hid_to_vk`
//! **是同一张表**（[`beam_input::hid`]），两边不会漂移。
//!
//! ## 不回采「注入事件」
//!
//! `LLKHF_INJECTED` / `LLMHF_INJECTED` 标记的事件是**别的程序注入**的
//! （包括本机另一端的主机注入）⇒ 一律跳过。
//! 否则会形成回环：注入 → 采集 → 回传 → 再注入 → …
//!
//! ## 线程模型
//!
//! - **采集线程**：装钩子 + 跑消息循环（§6「输入用专用 OS 线程」）；
//! - **事件队列**：采集线程写、会话线程读（`Mutex<VecDeque>`）；
//! - **会话线程**（`beam-session` 的 `pump`）通过
//!   [`beam_session::InputSource::poll_event`] 取事件。
//!
//! ## unsafe 策略（决策 D1）
//!
//! crate 根是 `#![forbid(unsafe_code)]`，只在确实要碰 Win32 裸 API 的地方
//! 局部 `#[allow(unsafe_code)]`：
//!
//! - 钩子回调：读 `KBDLLHOOKSTRUCT` / `MSLLHOOKSTRUCT`（`lParam` 是裸指针）；
//! - 装/卸钩子、跑消息循环：`SetWindowsHookExW` / `GetMessageW` 等。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use beam_proto::{ButtonAction, InputEvent, KeyAction, MouseButton};

#[cfg(windows)]
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};

/// 采集队列（采集线程写 → 会话线程读）。
type Queue = Arc<Mutex<VecDeque<InputEvent>>>;

// 钩子回调用的线程局部状态。
//
// **必须只有这一处声明**:`thread_local!` 写在两个不同的函数里就是
// **两个不同的变量** —— 钩子回调读到的那个永远是默认值
// （实测:注入事件被「跳过注入事件」的判断挡掉,一个都采不到）。
#[cfg(windows)]
thread_local! {
    /// 事件队列（钩子回调写 → 会话线程读）。
    static QUEUE: std::cell::RefCell<Option<Queue>> = const { std::cell::RefCell::new(None) };
    /// 已采集事件计数。
    static CAPTURED: std::cell::RefCell<Option<Arc<AtomicU64>>> =
        const { std::cell::RefCell::new(None) };
    /// 采集选项（是否收注入事件）。
    static OPTIONS: std::cell::RefCell<CaptureOptions> =
        const { std::cell::RefCell::new(CaptureOptions { capture_injected: false }) };
}

/// 采集源的选项。
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureOptions {
    /// 是否采集**注入事件**（别的程序用 `SendInput` 打进来的）。
    ///
    /// 默认**否**：注入事件多半来自本机另一端的主机回灌，
    /// 回采它会形成「注入 → 采集 → 回传 → 再注入」的回环。
    ///
    /// 打开它是为了**验证**用：验证入口需要自己注入一个键，
    /// 再确认它真的被钩子采到了（注入事件带 `LLKHF_INJECTED` 标记，
    /// 默认会被跳过）。
    pub capture_injected: bool,
}

/// Windows 客户端键鼠采集（低级钩子）。
///
/// 用法：
/// ```ignore
/// let capture = WindowsInputCapture::start()?;
/// runtime = runtime.with_input(Arc::new(capture));
/// ```
pub struct WindowsInputCapture {
    queue: Queue,
    stop: Arc<AtomicBool>,
    /// 钩子线程句柄（`Drop` 时停掉它）。
    ///
    /// 用 `Mutex` 包一层：`JoinHandle` 是 `Send` 但**不是 `Sync`**，
    /// 而采集源要以 `Arc<dyn InputSource>`（`Send + Sync`）交给会话层。
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 钩子是否真的装上了（装不上是最常见的失败：权限 / 钩子被策略拦）。
    installed: Arc<AtomicBool>,
    /// 装钩子失败的原因。
    error: Arc<Mutex<Option<String>>>,
    /// 已采集到的事件数（供验证入口观察「钩子真的在干活」）。
    captured: Arc<AtomicU64>,
    /// 选项（钩子回调在别的线程上跑，用线程局部传下去）。
    options: CaptureOptions,
    /// 采集线程的 **Win32** 线程 id（`GetCurrentThreadId`）。
    ///
    /// Rust 的 `std::thread::ThreadId` 拿不到 OS 线程 id（`as_u64` 还不稳定），
    /// 而 `PostThreadMessageW` 要的正是 OS 线程 id ⇒ 由采集线程自己填。
    thread_id: Arc<AtomicU32>,
}

impl std::fmt::Debug for WindowsInputCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsInputCapture")
            .field("installed", &self.installed.load(Ordering::SeqCst))
            .field("queued", &self.queue.lock().map(|q| q.len()).unwrap_or(0))
            .finish()
    }
}

impl WindowsInputCapture {
    /// 启动采集（装钩子 + 起采集线程）。
    ///
    /// 返回 `Err` 表示**连采集线程都起不来**；钩子本身装不上时**不**返回错误
    /// （原因记在 [`WindowsInputCapture::error`] 里），因为那更像是
    /// 「这台机器上钩子不可用」而不是「程序坏了」。
    pub fn start() -> Result<Self, String> {
        Self::start_with_options(CaptureOptions::default())
    }

    /// 用指定选项启动采集（§3.8）。
    pub fn start_with_options(options: CaptureOptions) -> Result<Self, String> {
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let installed = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let thread_id = Arc::new(AtomicU32::new(0));
        let captured = Arc::new(AtomicU64::new(0));
        // 就绪信号：`start()` 必须等钩子**真的装好**（或失败）才返回 ——
        // 否则调用方拿到的是「钩子还在装」的中间状态，会误判成失败。
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<bool>(1);

        let worker = {
            let queue = Arc::clone(&queue);
            let stop = Arc::clone(&stop);
            let installed = Arc::clone(&installed);
            let error = Arc::clone(&error);
            let thread_id = Arc::clone(&thread_id);
            let captured = Arc::clone(&captured);
            std::thread::Builder::new()
                .name("beam-input-capture".to_string())
                .spawn(move || {
                    hook_thread_main(CaptureShared {
                        queue,
                        stop,
                        installed,
                        error,
                        thread_id,
                        captured,
                        options,
                        ready: Some(ready_tx),
                    });
                })
                .map_err(|e| format!("起采集线程失败: {e}"))?
        };

        // 等钩子**真的装好**（或失败）再返回：否则调用方拿到的是
        // 「钩子还在装」的中间状态，会误判成失败。
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(true) => {}
            Ok(false) => {
                let reason = error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .unwrap_or_else(|| "低级钩子没能装上".to_string());
                return Err(reason);
            }
            Err(_) => {
                return Err("等待低级钩子安装超时(5 秒)".to_string());
            }
        }

        Ok(Self {
            queue,
            stop,
            worker: Mutex::new(Some(worker)),
            installed,
            error,
            thread_id,
            captured,
            options,
        })
    }

    /// 钩子是否真的装上了。
    pub fn is_installed(&self) -> bool {
        self.installed.load(Ordering::SeqCst)
    }

    /// 装钩子失败的原因（`None` = 没失败）。
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 已采集到的事件数（验证入口用它确认「钩子真的在干活」）。
    pub fn captured_count(&self) -> u64 {
        self.captured.load(Ordering::SeqCst)
    }

    /// 本次采集的选项。
    pub fn options(&self) -> CaptureOptions {
        self.options
    }

    /// 当前队列里攒了多少事件。
    pub fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// 停掉采集（卸钩子 + 结束采集线程）。
    /// 停掉采集（卸钩子 + 结束采集线程）。
    ///
    /// 用 `&self`：采集源要以 `Arc<dyn InputSource>` 交给会话层，
    /// 调用方拿不到 `&mut`。
    #[allow(unsafe_code)]
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // 让消息循环醒来：往线程消息队列里塞一条 WM_QUIT。
        #[cfg(windows)]
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id.load(Ordering::SeqCst),
                windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
        let worker = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

impl Drop for WindowsInputCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

impl beam_session::InputSource for WindowsInputCapture {
    fn poll_event(&self) -> Option<InputEvent> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }
}

/// 采集线程共享的状态（打包成结构体，避免参数过多）。
#[cfg(windows)]
struct CaptureShared {
    queue: Queue,
    stop: Arc<AtomicBool>,
    installed: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread_id: Arc<AtomicU32>,
    captured: Arc<AtomicU64>,
    options: CaptureOptions,
    ready: Option<std::sync::mpsc::SyncSender<bool>>,
}

/// 采集线程主体：装钩子 + 跑消息循环。
#[cfg(windows)]
#[allow(unsafe_code)]
fn hook_thread_main(shared: CaptureShared) {
    let CaptureShared {
        queue,
        stop,
        installed,
        error,
        thread_id,
        captured,
        options,
        ready: ready_tx,
    } = shared;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
        MSG, WH_KEYBOARD_LL, WH_MOUSE_LL,
    };

    // 钩子回调是 `extern "system"` 的普通函数 ⇒ 抓不到闭包。
    // 低级钩子的回调在**安装它的线程**上执行 ⇒ 用**线程局部**把队列传进去。
    QUEUE.with(|q| *q.borrow_mut() = Some(Arc::clone(&queue)));
    CAPTURED.with(|c| *c.borrow_mut() = Some(Arc::clone(&captured)));
    OPTIONS.with(|o| *o.borrow_mut() = options);
    // 记下本线程的 Win32 线程 id：`stop()` 要用它发 WM_QUIT 唤醒消息循环。
    thread_id.store(
        // SAFETY: GetCurrentThreadId 只是读当前线程 id，没有参数前置条件。
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
        Ordering::SeqCst,
    );

    // SAFETY: `SetWindowsHookExW` 的回调是纯 Win32 钩子，没有 Rust 侧的不变量；
    // 返回的 HHOOK 在下面被 `UnhookWindowsHookEx` 释放。
    unsafe {
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0);
        if let Err(e) = &kb {
            *error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("安装键盘低级钩子失败: {e}"));
        }
        let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0);
        if let Err(e) = &ms {
            *error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("安装鼠标低级钩子失败: {e}"));
        }
        let ok = kb.is_ok() || ms.is_ok();
        installed.store(ok, Ordering::SeqCst);
        if let Some(tx) = ready_tx {
            let _ = tx.send(ok);
        }

        // 消息循环：低级钩子靠它被调度。
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                // 0 = 收到 WM_QUIT；负数 = 出错。
                break;
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if let Ok(hook) = kb {
            let _ = UnhookWindowsHookEx(hook);
        }
        if let Ok(hook) = ms {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
}

/// 非 Windows 平台没有 Win32 钩子 ⇒ 采集线程直接返回（什么也不采）。
#[cfg(not(windows))]
fn hook_thread_main(shared: CaptureShared) {
    let CaptureShared {
        stop,
        installed,
        error,
        ready,
        ..
    } = shared;
    *error.lock().unwrap_or_else(|e| e.into_inner()) =
        Some("非 Windows 平台没有 Win32 钩子,客户端采集不可用".to_string());
    installed.store(false, Ordering::SeqCst);
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 键盘低级钩子回调。
#[cfg(windows)]
#[allow(unsafe_code)]
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
        WM_SYSKEYUP,
    };

    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let msg = wparam.0 as u32;
    let action = match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => KeyAction::Down,
        WM_KEYUP | WM_SYSKEYUP => KeyAction::Up,
        _ => return unsafe { CallNextHookEx(None, code, wparam, lparam) },
    };
    // SAFETY: `lParam` 在低级键盘钩子里指向一个有效的 `KBDLLHOOKSTRUCT`。
    let kb = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    // 别的程序注入的事件（LLKHF_INJECTED）不回采 ⇒ 不会形成「注入→采集→回传」回环。
    if kb.flags.contains(LLKHF_INJECTED) && !capture_injected_now() {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let vk = kb.vkCode;
    // 键码翻译：Win32 虚拟键码 → USB HID 用法 ID（§3.8）。
    let Some(hid) = beam_input::hid::vk_to_hid(vk) else {
        // 表里没有的键：跳过（**不静默丢弃整条流**，只是这一个键不转发）。
        tracing::debug!(vk, "HID 表里没有这个虚拟键码,跳过该按键");
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    };
    push_event(InputEvent::Key { code: hid, action });
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 鼠标低级钩子回调。
#[cfg(windows)]
#[allow(unsafe_code)]
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, LLMHF_INJECTED, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN,
        WM_XBUTTONUP,
    };

    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let msg = wparam.0 as u32;
    // SAFETY: `lParam` 在低级鼠标钩子里指向一个有效的 `MSLLHOOKSTRUCT`。
    let ms = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
    if ms.flags & LLMHF_INJECTED != 0 && !capture_injected_now() {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = normalize_point(ms.pt.x, ms.pt.y);
            push_event(InputEvent::MouseMove { x, y });
        }
        WM_LBUTTONDOWN => push_event(InputEvent::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Press,
        }),
        WM_LBUTTONUP => push_event(InputEvent::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Release,
        }),
        WM_RBUTTONDOWN => push_event(InputEvent::MouseButton {
            button: MouseButton::Right,
            action: ButtonAction::Press,
        }),
        WM_RBUTTONUP => push_event(InputEvent::MouseButton {
            button: MouseButton::Right,
            action: ButtonAction::Release,
        }),
        WM_MBUTTONDOWN => push_event(InputEvent::MouseButton {
            button: MouseButton::Middle,
            action: ButtonAction::Press,
        }),
        WM_MBUTTONUP => push_event(InputEvent::MouseButton {
            button: MouseButton::Middle,
            action: ButtonAction::Release,
        }),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            // 侧键：`mouseData` 的高位是 XBUTTON1 / XBUTTON2。
            let x1 = (ms.mouseData >> 16) & 0x0001 != 0;
            let button = if x1 { MouseButton::X1 } else { MouseButton::X2 };
            let action = if msg == WM_XBUTTONDOWN {
                ButtonAction::Press
            } else {
                ButtonAction::Release
            };
            push_event(InputEvent::MouseButton { button, action });
        }
        _ => {
            // 滚轮：`mouseData` 是 WHEEL_DELTA（120）刻度。
            const WHEEL_DELTA: u32 = 120;
            if msg == 0x020A || msg == 0x020B {
                // WM_MOUSEWHEEL / WM_MOUSEHWHEEL
                let notches = (ms.mouseData as i32) / WHEEL_DELTA as i32;
                let dy = notches << 16;
                let dx = if msg == 0x020B { notches << 16 } else { 0 };
                if dx != 0 || dy != 0 {
                    push_event(InputEvent::Scroll { dx, dy });
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 当前是否采集注入事件（线程局部：钩子回调在采集线程上跑）。
#[cfg(windows)]
fn capture_injected_now() -> bool {
    OPTIONS.with(|o| o.borrow().capture_injected)
}

/// 已采集事件数（线程局部）。
#[cfg(windows)]
fn bump_captured() {
    CAPTURED.with(|c| {
        if let Some(counter) = c.borrow().as_ref() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
}

/// 把一个采集到的事件放进队列（钩子回调 → 线程局部队列）。
#[cfg(windows)]
fn push_event(ev: InputEvent) {
    bump_captured();
    QUEUE.with(|q| {
        if let Some(queue) = q.borrow().as_ref() {
            let mut guard = queue.lock().unwrap_or_else(|e| e.into_inner());
            // 队列有界：采集比回传快时，丢**最旧**的（丢最新的会让时间轴倒退）。
            const MAX_QUEUE: usize = 256;
            if guard.len() >= MAX_QUEUE {
                guard.pop_front();
            }
            guard.push_back(ev);
        }
    });
}

/// 屏幕坐标 → 协议侧的归一化坐标（`0..=65535`，§3.8）。
///
/// 口径与主机侧注入一致：**主显示器**的宽高
/// （注入侧不加 `MOUSEEVENTF_VIRTUALDESK` ⇒ 协议说的是「屏幕」不是「虚拟桌面」）。
#[cfg(windows)]
#[allow(unsafe_code)]
fn normalize_point(x: i32, y: i32) -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: `GetSystemMetrics` 是纯查询，没有参数前置条件。
    let w = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(1);
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(1);
    let nx = (x.clamp(0, w - 1) as i64 * 65535 / (w - 1).max(1) as i64) as i32;
    let ny = (y.clamp(0, h - 1) as i64 * 65535 / (h - 1).max(1) as i64) as i32;
    (nx, ny)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_bounded_and_drops_the_oldest() {
        // 队列语义：攒太多时丢**最旧**的（丢最新的会让时间轴倒退）。
        let q: Queue = Arc::new(Mutex::new(VecDeque::new()));
        const MAX_QUEUE: usize = 256;
        for i in 0..(MAX_QUEUE + 10) {
            let mut guard = q.lock().unwrap();
            if guard.len() >= MAX_QUEUE {
                guard.pop_front();
            }
            guard.push_back(InputEvent::MouseMove { x: i as i32, y: 0 });
        }
        let guard = q.lock().unwrap();
        assert_eq!(guard.len(), MAX_QUEUE);
        // 最旧的 10 个被丢掉了。
        assert_eq!(guard[0], InputEvent::MouseMove { x: 10, y: 0 });
    }

    #[test]
    fn key_codes_are_translated_through_the_shared_hid_table() {
        // §3.8：协议侧是 HID 用法 ID；采集侧给的是虚拟键码 ⇒ 走同一张表。
        assert_eq!(beam_input::hid::vk_to_hid(0x41), Some(0x04)); // 'A' → HID 0x04
        assert_eq!(beam_input::hid::vk_to_hid(0x70), Some(0x39)); // F1 → HID 0x39
        assert_eq!(beam_input::hid::vk_to_hid(0xFFFF), None);
    }

    #[cfg(windows)]
    #[test]
    #[allow(unsafe_code)]
    fn normalized_coordinates_land_in_the_protocol_range() {
        let (x, y) = normalize_point(0, 0);
        assert_eq!((x, y), (0, 0));
        let w = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
            )
        };
        let (x, _) = normalize_point(w - 1, 0);
        assert!((0..=65535).contains(&x), "归一化坐标必须落在 0..=65535");
    }
}
