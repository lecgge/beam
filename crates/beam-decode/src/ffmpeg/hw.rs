//! D3D11VA 硬件设备上下文：让 ffmpeg 的解码器**复用调用方给的 D3D11 设备**。
//!
//! ## 跨 crate 设备契约（硬约束）
//!
//! D3D11 **不允许跨设备拷贝**。渲染层的 `D3D11Renderer::device()` 返回的是
//! **渲染层自己创建的 D3D11 设备**，交换链与后备缓冲都在它上面。
//! 解码出来的纹理必须创建在**同一个设备**上，否则：
//!
//! - 渲染层无法零拷贝地把它拷进后备缓冲；
//! - D3D11 本身也会拒绝跨设备拷贝。
//!
//! 因此本 crate **不自己 `CreateDevice`**，而是要求调用方把渲染层的设备传进来
//! （见 [`crate::decoder::D3D11VaDecoder::open_with_device`]）。
//! §4 规定 L1 之间只有 net→pair 可以横向依赖，所以 beam-decode 不能依赖
//! beam-render，契约只能通过 `ID3D11Device` 这个平台类型来表达。
//!
//! ## FFmpeg 侧的机制
//!
//! `libavutil/hwcontext_d3d11va.h`（FFmpeg 6.1 原文）：
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
//! **实测**：`av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA)` 已经替我们分配好了
//! 这个结构（`device` 字段为 NULL），我们只填 `device`，**不能覆盖 `hwctx` 指针本身**
//! （否则 ffmpeg 的释放路径会用 `av_free` 释放 Rust 分配的内存 → 堆破坏）。
//! 而且 ffmpeg 装上的释放回调是空的，**设备引用由我们负责释放**。

use std::ffi::c_void;

use ffmpeg_next::sys as ffi;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::error::DecError;

/// ffmpeg 的 `AVD3D11VADeviceContext`（只声明布局，本 crate 只写 `device` 字段）。
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

/// ffmpeg 的 D3D11VA 硬件设备上下文（建在调用方给的设备上）。
pub struct HwDevice {
    /// `AVBufferRef`，指向 `AVHWDeviceContext`。
    device_ref: *mut ffi::AVBufferRef,
    /// 我们自己持有的 `ID3D11Device` 引用。
    ///
    /// ffmpeg 的释放回调是空的，**它不会**替我们释放这个引用，所以这一份由我们持有。
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
    /// 用调用方提供的 D3D11 设备建立 ffmpeg 的硬件设备上下文。
    ///
    /// **必须**传入渲染层那个设备（`D3D11Renderer::device()`），
    /// 否则解码纹理与交换链不在同一个设备上，帧链路是断的。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的 hwcontext FFI（av_hwdevice_ctx_alloc/init）并把 COM 指针交给它"
    )]
    pub fn from_d3d11_device(device: &ID3D11Device) -> Result<Self, DecError> {
        let mut device_ref =
            unsafe { ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA) };
        if device_ref.is_null() {
            return Err(DecError::DecoderUnavailable);
        }

        // 克隆即 AddRef：ffmpeg 不会释放这个引用（它的释放回调是空的）。
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
            return Err(DecError::DecoderUnavailable);
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
