//! ffmpeg 后端：D3D11VA 硬解（§3.10）。
//!
//! ## 流水线
//!
//! ```text
//! AssembledFrame(H.264/HEVC 码流)
//!   └─ AVPacket ─► AVCodecContext(get_format → AV_PIX_FMT_D3D11)
//!                      └─ D3D11VA 硬件解码 ─► D3D11 纹理 ─► DecodedFrame.surface
//! ```
//!
//! 走的是 FFmpeg 公开的「`hw_device_ctx` + `get_format` 回调」路线
//! （`libavcodec/avcodec.h` 第 2670 行：「an API user can set
//! AVCodecContext.hw_device_ctx, which sets up AVCodecContext.hw_frames_ctx
//! fully automatically」），不需要碰 ffmpeg 的内部结构。
//!
//! **验证状态**：「D3D11VA 硬解」已在本机真机验证
//! （见 `examples/d3d11va_decode_smoke.rs`）。

use std::ffi::{c_void, CString};

use beam_proto::{AssembledFrame, CodecKind, PlatformHandle};
use ffmpeg_next::sys as ffi;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

use crate::decoder::DecodedSurface;
use crate::error::DecError;
use crate::ffmpeg::hw::HwDevice;
use crate::slot::DecodedOutput;

/// MSVC / libc 的 `EAGAIN`（两平台取值一致）。
const EAGAIN: i32 = 11;

/// 编解码器在 ffmpeg 里的解码器名。
pub fn decoder_name(codec: CodecKind) -> Option<&'static str> {
    match codec {
        CodecKind::Hevc => Some("hevc"),
        CodecKind::H264 => Some("h264"),
        CodecKind::Av1 => Some("av1"),
    }
}

/// 已打开的 ffmpeg D3D11VA 硬解上下文。
pub struct FfmpegD3D11VaDecoder {
    /// `AVCodecContext`。
    ctx: *mut ffi::AVCodecContext,
    /// 解码纹理所在的 D3D11 设备（**必须**与渲染层的是同一个）。
    device: ID3D11Device,
    /// ffmpeg 的 D3D11VA 设备上下文（保活：解码上下文引用着它）。
    _hw: Option<HwDevice>,
    /// 目标编解码器。
    codec: CodecKind,
    /// 解码宽（由输出帧带出）。
    width: u32,
    /// 解码高。
    height: u32,
}

// SAFETY（`Send`）：ffmpeg 的 `AVCodecContext` 与 `HwDevice` 都不是线程安全的，
// 但**本类型始终只被一个消费者独占**：
//
// - `VideoDecoder::decode(&mut self, ...)` 要求独占引用 ⇒ 同一时刻只有一个线程在用它；
// - 会话层把解码器整体**移动**进帧消费者（`DecodeRenderConsumer`），
//   移动之后只在会话的取帧循环那一个线程上调用。
//
// 因此「移动所有权」是安全的，「并发使用」不可能发生 —— 后者被 `&mut self` 排除。
#[allow(
    unsafe_code,
    reason = "解码上下文始终被独占借用（&mut self），跨线程只移动所有权，不存在并发使用"
)]
unsafe impl Send for FfmpegD3D11VaDecoder {}

/// 自建一个 D3D11 设备（**仅供「单独跑解码器」的场景**）。
///
/// 生产路径必须用 [`FfmpegD3D11VaDecoder::open_on_device`] 复用渲染层的设备。
#[allow(unsafe_code, reason = "需要调用 D3D11CreateDevice 创建 COM 对象")]
fn create_own_device() -> Result<(ID3D11Device, ID3D11DeviceContext), DecError> {
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    };

    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device as *mut Option<ID3D11Device>),
            None,
            Some(&mut context as *mut Option<ID3D11DeviceContext>),
        )
    }
    .map_err(|_| DecError::DecoderUnavailable)?;
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(DecError::DecoderUnavailable),
    }
}

impl std::fmt::Debug for FfmpegD3D11VaDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfmpegD3D11VaDecoder")
            .field("codec", &self.codec)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl FfmpegD3D11VaDecoder {
    /// 打开硬解器（**自建设备**）。
    ///
    /// ⚠️ **生产路径请用 [`Self::open_on_device`]**：自建的设备与渲染层的交换链
    /// 不在同一个 D3D11 设备上，解码纹理无法零拷贝地交给渲染层
    /// （D3D11 禁止跨设备拷贝）。这个入口只给「单独跑解码器」的场景用。
    ///
    /// - 本机没有可用的 D3D11VA 解码器 → `E10001`。
    /// - ffmpeg 构建里没有该解码器 → `E10001`。
    #[allow(unsafe_code, reason = "需要创建 D3D11 设备并调用 ffmpeg 的编解码 FFI")]
    pub fn open(codec: CodecKind) -> Result<Self, DecError> {
        let (device, _context) = create_own_device()?;
        Self::open_on_device(codec, &device)
    }

    /// 打开硬解器，**复用调用方提供的 D3D11 设备**。
    ///
    /// 这是**生产路径**：调用方应当传入渲染层自己的设备
    /// （`beam_render::D3D11Renderer::device()`），
    /// 保证「解码纹理」与「交换链后备缓冲」在同一个 `ID3D11Device` 上——
    /// D3D11 不允许跨设备拷贝，设备不一致时帧链路是断的。
    ///
    /// §4 规定 L1 之间不能横向依赖，所以 beam-decode 不能直接依赖 beam-render，
    /// 契约只能通过 `ID3D11Device` 这个平台类型来表达。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的 hwdevice / 编解码 FFI 并管理原生句柄"
    )]
    pub fn open_on_device(codec: CodecKind, device: &ID3D11Device) -> Result<Self, DecError> {
        let name = decoder_name(codec).ok_or(DecError::DecoderUnavailable)?;
        let codec_ptr = find_decoder(name);
        if codec_ptr.is_null() {
            return Err(DecError::DecoderUnavailable);
        }

        // D3D11VA 硬件设备上下文，建在**调用方给的设备**上。
        let hw = HwDevice::from_d3d11_device(device)?;

        let mut ctx = unsafe { ffi::avcodec_alloc_context3(codec_ptr) };
        if ctx.is_null() {
            return Err(DecError::DecoderUnavailable);
        }

        let ret = unsafe {
            // 把 `AVBufferRef` **加一份引用**交给 libavcodec。
            //
            // `avctx->hw_device_ctx` 与 `avci->hw_device_ctx` 各持一份引用，
            // `avcodec_free_context` 会把它们都 unref 掉。
            // 如果只是把指针**塞进去**（不加引用），libavcodec 释放的
            // 就是我们自己那一份 ⇒ 之后 `HwDevice::drop` 再 unref 一次
            // ⇒ use-after-free ⇒ 堆破坏（实测：退出会话时
            // STATUS_HEAP_CORRUPTION / ACCESS_VIOLATION）。
            let new_ref = ffi::av_buffer_ref(hw.as_ptr());
            if new_ref.is_null() {
                ffi::AVERROR(ffi::ENOMEM)
            } else {
                (*ctx).hw_device_ctx = new_ref;
                (*ctx).get_format = Some(get_format_d3d11);
                ffi::avcodec_open2(ctx, codec_ptr, std::ptr::null_mut())
            }
        };
        if ret < 0 {
            // `avcodec_free_context` 会 unref `avctx->hw_device_ctx`（上面加的那一份）；
            // 我们自己那一份由 `HwDevice::drop` 释放。
            unsafe {
                ffi::avcodec_free_context(&mut ctx);
            }
            return Err(DecError::DecoderUnavailable);
        }

        Ok(Self {
            ctx,
            device: device.clone(),
            _hw: Some(hw),
            codec,
            width: 0,
            height: 0,
        })
    }

    /// 目标编解码器。
    pub fn codec(&self) -> CodecKind {
        self.codec
    }

    /// 解码纹理所在的 D3D11 设备。
    ///
    /// 调用方（渲染层）可以拿它和自己的设备比对，确认**设备一致**——
    /// D3D11 不允许跨设备拷贝，设备不一致时帧链路是断的。
    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// 解码一帧。
    ///
    /// 返回的 [`DecodedOutput`] 里带着 D3D11 纹理的保活引用：
    /// 渲染线程取走之前纹理不会被释放。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的解码 FFI，并借用 AVFrame 里的 D3D11 纹理指针"
    )]
    pub fn decode(&mut self, f: &AssembledFrame) -> Result<DecodedOutput, DecError> {
        let mut pkt = unsafe { ffi::av_packet_alloc() };
        if pkt.is_null() {
            return Err(DecError::DecodeFailed {
                reason: "无法分配 AVPacket".to_string(),
            });
        }
        // 用带引用计数的缓冲承载码流：解码器可能延后消费，裸指针不安全。
        let len = f.payload.len().min(i32::MAX as usize) as i32;
        let ret = unsafe { ffi::av_new_packet(pkt, len) };
        if ret < 0 {
            unsafe { ffi::av_packet_free(&mut pkt) };
            return Err(DecError::DecodeFailed {
                reason: "无法分配码流缓冲".to_string(),
            });
        }
        unsafe {
            if len > 0 {
                std::ptr::copy_nonoverlapping(f.payload.as_ptr(), (*pkt).data, len as usize);
            }
            // 时基按 1/90000（微秒换算），与 §6 的微秒时间戳对齐。
            (*pkt).pts = f.pts_us as i64 * 90;
        }

        let ret = unsafe { ffi::avcodec_send_packet(self.ctx, pkt) };
        unsafe { ffi::av_packet_free(&mut pkt) };
        if ret < 0 && ret != ffi::AVERROR(EAGAIN) {
            return Err(DecError::DecodeFailed {
                reason: format!("送包失败（ffmpeg 错误码 {ret}）"),
            });
        }

        let mut frame = unsafe { ffi::av_frame_alloc() };
        if frame.is_null() {
            return Err(DecError::DecodeFailed {
                reason: "无法分配 AVFrame".to_string(),
            });
        }
        let ret = unsafe { ffi::avcodec_receive_frame(self.ctx, frame) };
        if ret < 0 {
            unsafe { ffi::av_frame_free(&mut frame) };
            if ret == ffi::AVERROR(EAGAIN) || ret == ffi::AVERROR_EOF {
                // 暂时没有可输出的帧（多帧重排序时会出现）。
                return Err(DecError::DecodeFailed {
                    reason: "解码器暂未产出帧".to_string(),
                });
            }
            return Err(DecError::DecodeFailed {
                reason: format!("取帧失败（ffmpeg 错误码 {ret}）"),
            });
        }

        let width = unsafe { (*frame).width } as u32;
        let height = unsafe { (*frame).height } as u32;
        self.width = width;
        self.height = height;

        // D3D11 硬件帧：data[0] 是 ID3D11Texture2D*，data[1] 是数组纹理下标。
        let surface = surface_from_frame(frame);
        let handle = surface
            .as_ref()
            .map(|s| s.handle())
            .unwrap_or(PlatformHandle::NONE);

        unsafe { ffi::av_frame_unref(frame) };
        unsafe { ffi::av_frame_free(&mut frame) };

        Ok(DecodedOutput {
            frame: beam_proto::DecodedFrame {
                frame_id: f.frame_id,
                pts_us: f.pts_us,
                capture_ts_us: f.capture_ts_us,
                width,
                height,
                is_key: f.is_key,
                surface: handle,
            },
            surface,
        })
    }
}

impl Drop for FfmpegD3D11VaDecoder {
    #[allow(
        unsafe_code,
        reason = "释放 AVCodecContext 是 FFI 调用，必须与 avcodec_alloc_context3 成对"
    )]
    fn drop(&mut self) {
        unsafe {
            ffi::avcodec_free_context(&mut self.ctx);
        }
    }
}

/// `get_format` 回调：优先选 D3D11 硬件帧。
///
/// FFmpeg 的标准 D3D11VA 解码写法：候选列表里有 `AV_PIX_FMT_D3D11` 就选它，
/// 否则返回 `AV_PIX_FMT_NONE` 让 libavcodec 走软件回退。
#[allow(
    unsafe_code,
    reason = "这是给 ffmpeg 调用的 C 回调，必须按 FFI 签名声明"
)]
unsafe extern "C" fn get_format_d3d11(
    _ctx: *mut ffi::AVCodecContext,
    fmt: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let mut i = 0isize;
    loop {
        let cur = unsafe { *fmt.offset(i) };
        if cur == ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            return ffi::AVPixelFormat::AV_PIX_FMT_NONE;
        }
        if cur == ffi::AVPixelFormat::AV_PIX_FMT_D3D11 {
            return cur;
        }
        i += 1;
    }
}

/// 从 D3D11 硬件帧里借用出纹理（AddRef 一份供渲染阶段保活）。
#[allow(
    unsafe_code,
    reason = "需要把 AVFrame 里的裸指针借用成 COM 接口并 AddRef"
)]
fn surface_from_frame(frame: *mut ffi::AVFrame) -> Option<DecodedSurface> {
    unsafe {
        let raw: *mut c_void = (*frame).data[0].cast::<c_void>();
        if raw.is_null() {
            return None;
        }
        // `from_raw_borrowed` 不接管所有权；`cloned()` 会 AddRef 一次。
        let texture: ID3D11Texture2D = ID3D11Texture2D::from_raw_borrowed(&raw).cloned()?;
        Some(DecodedSurface::from_texture(texture))
    }
}

/// 按名字查找 ffmpeg 解码器。
#[allow(unsafe_code, reason = "需要调用 ffmpeg 的解码器查找 FFI")]
pub fn find_decoder(name: &str) -> *const ffi::AVCodec {
    let cname = match CString::new(name) {
        Ok(v) => v,
        Err(_) => return std::ptr::null(),
    };
    unsafe { ffi::avcodec_find_decoder_by_name(cname.as_ptr()) }
}
