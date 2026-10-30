//! 独立原生串流窗口（§3.10 / 产品方案第 45 行：「Windows 作为客户端用独立原生
//! D3D11/wgpu 窗口渲染，**不走 WebView**」）。
//!
//! ## 为什么窗口住在渲染层
//!
//! §3.10 里 `beam-render` 的职责是「上屏」——而 D3D11 flip-model 交换链
//! **必须**挂在一个真实的 `HWND` 上。窗口与交换链是一对，
//! 把它们放在一起才不会出现「交换链建好了但宿主窗口没了」这类生命周期事故。
//!
//! ## 为什么是**独立顶层窗口**，不是 Tauri 窗口的子控件
//!
//! WebView2 的渲染子窗口铺满整个客户区；原生窗口做成它的**子窗口**会被盖住。
//! 因此这里建的是**顶层窗口**（`WS_OVERLAPPEDWINDOW`），
//! 由上层决定何时显示、放在哪里。
//!
//! ## 窗口模型
//!
//! - 窗口在**自己的线程**上创建，并由该线程跑消息循环（`GetMessageW`）——
//!   Win32 要求窗口的消息泵与创建线程一致；
//! - [`VideoWindow::hwnd`] 交出去的是 `HWND` 的整数值，
//!   与 [`crate::renderer::WindowHandle`] 对齐。
//!
//! ## 生命周期
//!
//! `Drop` 会给窗口发 `WM_CLOSE`，由窗口自己的线程执行
//! `DestroyWindow` → `WM_DESTROY` → `PostQuitMessage`，
//! 消息循环退出、线程结束。**不跨线程调用 `DestroyWindow`**（Win32 不允许）。
//!
//! ## §4 分层说明
//!
//! 本模块是 **L1 平台层**的一部分，因此 `beam-render` 只能被
//! `beam-host` / `beam-client` 引用（§4）。桌面端（L4）要窗口，
//! 只能经由 `beam-client`（L3）拿到 —— `beam_client::VideoWindow` 就是它的重导出。

use std::fmt;

/// 窗口创建失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowError {
    /// 人类可读的原因。
    pub reason: String,
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for WindowError {}

/// 独立原生串流窗口（句柄 + 消息泵线程）。
///
/// `Drop` 时自动关闭（发 `WM_CLOSE`，由窗口自己的线程销毁）。
#[derive(Debug)]
pub struct VideoWindow {
    imp: imp::VideoWindow,
}

impl VideoWindow {
    /// 创建串流窗口。
    ///
    /// - `title`：窗口标题。
    /// - `width` / `height`：客户区尺寸（像素）。
    pub fn create(title: &str, width: u32, height: u32) -> Result<Self, WindowError> {
        imp::VideoWindow::create(title, width, height)
            .map(|imp| Self { imp })
            .map_err(|reason| WindowError { reason })
    }

    /// 窗口句柄（Windows 上是 `HWND` 的整数值）。
    ///
    /// 交给 [`crate::renderer::WindowHandle`] 建交换链。
    pub fn hwnd(&self) -> usize {
        self.imp.hwnd()
    }
}

#[cfg(all(windows, feature = "d3d11"))]
mod imp {
    //! Windows 实现：原生窗口 + 专用消息泵线程。
    //!
    //! 本模块需要 unsafe：Win32 的窗口注册 / 创建 / 消息泵全是裸 API 调用。
    #![allow(
        unsafe_code,
        reason = "Win32 窗口 API（注册窗口类、建窗口、消息泵）本身是 unsafe 的裸调用"
    )]

    use std::sync::mpsc;
    use std::sync::OnceLock;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::HBRUSH;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostMessageW, PostQuitMessage, RegisterClassExW, ShowWindow, TranslateMessage, CS_HREDRAW,
        CS_VREDRAW, HCURSOR, HICON, MSG, SW_SHOW, WM_CLOSE, WM_DESTROY, WNDCLASSEXW,
        WS_OVERLAPPEDWINDOW,
    };

    /// 窗口类名（进程内注册一次）。
    const CLASS_NAME: PCWSTR = w!("BeamVideoWindow");

    /// `CW_USEDEFAULT`（Win32 的「系统默认位置」常量）。
    const CW_USEDEFAULT: i32 = -2147483648;

    /// 窗口过程。
    ///
    /// - `WM_CLOSE` → `DestroyWindow`（**必须**在创建线程上调用）；
    /// - `WM_DESTROY` → `PostQuitMessage`，让消息循环退出；
    /// - 其余交回系统默认处理。
    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(hwnd) };
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// 注册窗口类（进程内只做一次）。
    ///
    /// 缓存里存的是模块句柄的**整数值**（`HINSTANCE` 内部是裸指针，
    /// 不是 `Send`/`Sync`，不能放进 `OnceLock`）。
    fn register_class() -> Result<usize, String> {
        static ONCE: OnceLock<Result<usize, String>> = OnceLock::new();
        let result = ONCE.get_or_init(|| {
            let instance =
                unsafe { GetModuleHandleW(None) }.map_err(|e| format!("取模块句柄失败: {e}"))?;
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: windows::Win32::Foundation::HINSTANCE(instance.0),
                hIcon: HICON::default(),
                hCursor: HCURSOR::default(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: CLASS_NAME,
                hIconSm: HICON::default(),
            };
            // 返回 0 表示失败（重复注册会失败，但此时类已经可用）。
            let atom = unsafe { RegisterClassExW(&wc) };
            if atom == 0 {
                return Err("注册窗口类失败".to_string());
            }
            Ok(instance.0 as usize)
        });
        result.clone()
    }

    /// 原生窗口 + 消息泵线程。
    pub struct VideoWindow {
        hwnd: usize,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl std::fmt::Debug for VideoWindow {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("VideoWindow")
                .field("hwnd", &self.hwnd)
                .finish()
        }
    }

    impl VideoWindow {
        /// 创建窗口；窗口的消息泵跑在新建的线程上。
        pub fn create(title: &str, width: u32, height: u32) -> Result<Self, String> {
            let instance = register_class()?;
            let title = title.to_string();
            let (tx, rx) = mpsc::channel::<Result<usize, String>>();

            let thread = std::thread::Builder::new()
                .name("beam-video-window".to_string())
                .spawn(move || {
                    let hwnd = unsafe {
                        CreateWindowExW(
                            Default::default(),
                            CLASS_NAME,
                            &windows::core::HSTRING::from(title),
                            WS_OVERLAPPEDWINDOW,
                            CW_USEDEFAULT,
                            CW_USEDEFAULT,
                            width as i32,
                            height as i32,
                            None,
                            None,
                            Some(windows::Win32::Foundation::HINSTANCE(
                                instance as *mut core::ffi::c_void,
                            )),
                            None,
                        )
                    };
                    let hwnd = match hwnd {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = tx.send(Err(format!("创建原生窗口失败: {e}")));
                            return;
                        }
                    };
                    let _ = tx.send(Ok(hwnd.0 as usize));
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_SHOW);
                    }
                    // 消息泵：与窗口同线程。
                    let mut msg = MSG::default();
                    loop {
                        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                        if ret.0 <= 0 {
                            break;
                        }
                        unsafe {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                })
                .map_err(|e| format!("起窗口线程失败: {e}"))?;

            match rx.recv() {
                Ok(Ok(hwnd)) => Ok(Self {
                    hwnd,
                    thread: Some(thread),
                }),
                Ok(Err(reason)) => Err(reason),
                Err(_) => Err("窗口线程在回传句柄前就结束了".to_string()),
            }
        }

        /// 窗口句柄（`HWND` 的整数值）。
        pub fn hwnd(&self) -> usize {
            self.hwnd
        }
    }

    impl Drop for VideoWindow {
        fn drop(&mut self) {
            // 让窗口自己的线程去销毁它（Win32 要求 DestroyWindow 在创建线程上调用）。
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(not(all(windows, feature = "d3d11")))]
mod imp {
    //! 非 Windows 平台或未启用 `d3d11` feature 时的占位。
    //!
    //! **如实报错**，不静默降级 —— 上层必须知道「没有原生窗口」。

    /// 占位实现。
    #[derive(Debug)]
    pub struct VideoWindow;

    impl VideoWindow {
        /// 永远失败。
        pub fn create(_title: &str, _width: u32, _height: u32) -> Result<Self, String> {
            Err(
                "当前构建未启用 `d3d11` feature（或不是 Windows 平台），无法创建原生串流窗口"
                    .to_string(),
            )
        }

        /// 永远返回 0（没有窗口）。
        pub fn hwnd(&self) -> usize {
            0
        }
    }
}
