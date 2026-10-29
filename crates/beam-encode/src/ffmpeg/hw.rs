//! D3D11 硬件上下文：把本进程的 `ID3D11Device` 交给 ffmpeg，并建立 D3D11 帧池。
//!
//! §3.6 要求「NVENC/AMF/QSV 硬编（ffmpeg-next 封装，**D3D11 硬件帧上下文**）」。
//! 要让 ffmpeg 的编码器直接吃我们自己的 D3D11 纹理，就必须让 ffmpeg 的硬件设备
//! 上下文建在**同一个 `ID3D11Device`** 上——否则纹理无法跨设备共享，
//! `CopyResource` / `CopySubresourceRegion` 都是同设备操作。
//!
//! FFmpeg 为此提供了公开机制（`libavutil/hwcontext_d3d11va.h`，FFmpeg 6.1 原文）：
//!
//! ```text
//! /** This struct is allocated as AVHWDeviceContext.hwctx */
//! typedef struct AVD3D11VADeviceContext {
//!     ID3D11Device        *device;   // Must be set by the user.
//!     ID3D11DeviceContext *device_context;
//!     ...
//! } AVD3D11VADeviceContext;
//! ```
//!
//! 也就是：`av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA)` 之后，
//! 把 `hwdev->hwctx` 指向一个填好了 `device` 的 `AVD3D11VADeviceContext`，
//! 再 `av_hwdevice_ctx_init()`。ffmpeg-sys-next 没有绑定这个头文件，
//! 因此本模块按头文件布局本地声明一份。
//!
//! **验证状态**：本批没有送入任何真实抓屏纹理，因此这段代码**未经真实硬件验证**。

use std::ffi::c_void;

use ffmpeg_next::sys as ffi;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::error::EncError;

/// ffmpeg 的 `AVD3D11VADeviceContext`（`libavutil/hwcontext_d3d11va.h`）。
///
/// 头文件原文（FFmpeg 6.1）：
/// > This struct is allocated as AVHWDeviceContext.hwctx
/// > `ID3D11Device *device;` … Must be set by the user. This is the only mandatory field
///
/// **实测（FFmpeg 6.1）**：`av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA)`
/// 已经替我们分配好了这个结构（挂在 `AVHWDeviceContext.hwctx` 上，`device` 为 NULL）。
/// 因此我们**只填 `device` 字段**，绝不能覆盖 `hwctx` 指针本身——
///
/// - 覆盖它会让 ffmpeg 的释放路径用 `av_free` 释放一块 Rust 分配的内存 → 堆破坏；
/// - 而且 `av_hwdevice_ctx_init()` 会直接解引用 `hwctx->device`，为 NULL 时必崩。
///
/// ffmpeg-sys-next 没有绑定这个头文件，因此这里按头文件布局本地声明一份。
#[repr(C)]
struct AvD3D11VaDeviceContext {
    /// `ID3D11Device *`：唯一必填字段。
    device: *mut c_void,
    /// `ID3D11DeviceContext *`：留空则由 ffmpeg 从 `device` 推导。
    #[allow(dead_code, reason = "只声明布局，本 crate 只写 device 字段")]
    device_context: *mut c_void,
    /// `ID3D11VideoDevice *`：留空则由 ffmpeg 推导。
    #[allow(dead_code, reason = "只声明布局，本 crate 只写 device 字段")]
    video_device: *mut c_void,
    /// `ID3D11VideoContext *`：留空则由 ffmpeg 推导。
    #[allow(dead_code, reason = "只声明布局，本 crate 只写 device 字段")]
    video_context: *mut c_void,
    /// 加锁回调：留空则用 ffmpeg 的内部互斥量。
    #[allow(dead_code, reason = "只声明布局，本 crate 只写 device 字段")]
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    /// 解锁回调：留空则用 ffmpeg 的内部互斥量。
    #[allow(dead_code, reason = "只声明布局，本 crate 只写 device 字段")]
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    /// 锁回调的上下文。
    #[allow(dead_code, reason = "只声明布局，本 crate 只写 device 字段")]
    lock_ctx: *mut c_void,
}

/// ffmpeg 的 D3D11VA 硬件设备上下文。
pub struct HwDevice {
    /// `AVBufferRef`，指向 `AVHWDeviceContext`。
    device_ref: *mut ffi::AVBufferRef,
    /// 我们自己持有的 `ID3D11Device` 引用。
    ///
    /// 实测：`av_hwdevice_ctx_alloc()` 装上的释放回调是空的，ffmpeg **不会**替我们
    /// 释放这个设备引用，所以这一份必须由我们自己持有与释放。
    _device: ID3D11Device,
}

impl std::fmt::Debug for HwDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwDevice")
            .field("device_ref", &self.device_ref)
            .finish()
    }
}

impl HwDevice {
    /// 用调用方自己的 D3D11 设备建立 ffmpeg 的硬件设备上下文。
    ///
    /// 传入的设备通常来自抓屏纹理（`ID3D11Texture2D::GetDevice`），
    /// 这样颜色转换出来的 NV12 纹理与 ffmpeg 的帧池在**同一个设备**上，
    /// 才能用 `CopyResource` 互拷。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的 hwcontext FFI（av_hwdevice_ctx_alloc/init）并把 COM 指针交给它"
    )]
    pub fn from_d3d11_device(device: &ID3D11Device) -> Result<Self, EncError> {
        let mut device_ref =
            unsafe { ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA) };
        if device_ref.is_null() {
            return Err(EncError::NoHardwareEncoder);
        }

        // 克隆即 AddRef：ffmpeg 不会释放这个引用（它的释放回调是空的），
        // 所以这一份由我们自己持有与释放。
        let owned = device.clone();

        let ret = unsafe {
            let hwdev = (*device_ref).data as *mut ffi::AVHWDeviceContext;
            if hwdev.is_null() || (*hwdev).hwctx.is_null() {
                ffi::AVERROR(ffi::EINVAL)
            } else {
                // 只填 ffmpeg 已经分配好的结构体，不覆盖 hwctx 指针本身。
                let hwfc = (*hwdev).hwctx as *mut AvD3D11VaDeviceContext;
                (*hwfc).device = owned.as_raw();
                ffi::av_hwdevice_ctx_init(device_ref)
            }
        };
        if ret < 0 {
            unsafe { ffi::av_buffer_unref(&mut device_ref) };
            return Err(EncError::NoHardwareEncoder);
        }

        Ok(Self {
            device_ref,
            _device: owned,
        })
    }

    /// `AVBufferRef` 指针（`AVHWDeviceContext`）。
    pub fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.device_ref
    }
}

impl Drop for HwDevice {
    #[allow(
        unsafe_code,
        reason = "释放 ffmpeg 的 AVBufferRef 是 FFI 调用，必须在 Drop 里成对释放"
    )]
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.device_ref) };
    }
}

/// ffmpeg 的 D3D11 硬件帧池上下文（`AV_PIX_FMT_D3D11` / NV12 布局）。
pub struct HwFrames {
    /// `AVBufferRef`，指向 `AVHWFramesContext`。
    frames_ref: *mut ffi::AVBufferRef,
}

impl std::fmt::Debug for HwFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwFrames")
            .field("frames_ref", &self.frames_ref)
            .finish()
    }
}

impl HwFrames {
    /// 在给定设备上建立 D3D11 帧池。
    ///
    /// `initial_pool_size = 0` 让 ffmpeg 为每一帧单独分配纹理
    /// （而不是数组纹理），编码器逐帧取用更简单。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的 hwcontext FFI 并直接写 AVHWFramesContext 字段"
    )]
    pub fn new(device: &HwDevice, width: u32, height: u32) -> Result<Self, EncError> {
        let mut frames_ref = unsafe { ffi::av_hwframe_ctx_alloc(device.as_ptr()) };
        if frames_ref.is_null() {
            return Err(EncError::NoHardwareEncoder);
        }
        let ret = unsafe {
            let fc = (*frames_ref).data as *mut ffi::AVHWFramesContext;
            if fc.is_null() {
                ffi::AVERROR(ffi::EINVAL)
            } else {
                (*fc).format = ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
                // D3D11 表面背后的实际排布：8bit 4:2:0 NV12。
                (*fc).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
                (*fc).width = width as i32;
                (*fc).height = height as i32;
                (*fc).initial_pool_size = 0;
                ffi::av_hwframe_ctx_init(frames_ref)
            }
        };
        if ret < 0 {
            unsafe { ffi::av_buffer_unref(&mut frames_ref) };
            return Err(EncError::NoHardwareEncoder);
        }
        Ok(Self { frames_ref })
    }

    /// `AVBufferRef` 指针（`AVHWFramesContext`）。
    pub fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.frames_ref
    }
}

impl Drop for HwFrames {
    #[allow(
        unsafe_code,
        reason = "释放 ffmpeg 的 AVBufferRef 是 FFI 调用，必须在 Drop 里成对释放"
    )]
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.frames_ref) };
    }
}
