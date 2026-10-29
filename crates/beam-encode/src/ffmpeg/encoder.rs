//! ffmpeg 后端：真实的 NVENC / AMF / QSV 硬编（§3.6）。
//!
//! 本模块用 `ffmpeg-next` 8.1 的 FFI 层（`ffmpeg_next::sys`）建立编码器，
//! 因为硬件帧上下文（`hw_frames_ctx` / `hw_device_ctx`）与「参数不支持→降档」
//! 的重试逻辑都需要直接操作 `AVCodecContext`。
//!
//! ## 流水线
//!
//! ```text
//! 抓屏纹理 (RGBA, D3D11)
//!   └─ D3D11 VideoProcessor ──► NV12 纹理 (D3D11, 与 ffmpeg 帧池同设备)
//!                                  ├─ D3D11 硬件帧路径：CopyResource 进 ffmpeg 帧池
//!                                  └─ NV12 上传路径：回读内存，按 AV_PIX_FMT_NV12 送帧
//!                                       └─ NVENC / AMF / QSV ──► AVPacket ──► EncodedFrame.slices
//! ```
//!
//! ## 验证状态
//!
//! **「NVENC 出码流」已在本机真机验证**（§9 M-1 验证点②）：
//! 见 `examples/nvenc_encode_smoke.rs`——真的用 NVENC 编出 HEVC 码流，
//! 再用软件解码器把码流解回来核对分辨率。

use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicUsize, Ordering};

use beam_proto::{CodecKind, EncodedFrame};
use bytes::Bytes;
use ffmpeg_next::codec::flag::Flags;
use ffmpeg_next::sys as ffi;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

use crate::color::{read_back_nv12, RgbToNv12};
use crate::encoder::{
    select_encoder_kind, supports_codec, CapturedFrame, EncodeConfig, EncoderKind,
};
use crate::error::EncError;
use crate::ffmpeg::hw::{HwDevice, HwFrames};
use crate::params;

/// MSVC / libc 的 `EINVAL`（两平台取值一致）。
const EINVAL: i32 = 22;
/// MSVC / libc 的 `EAGAIN`（两平台取值一致）。
const EAGAIN: i32 = 11;
/// `AV_PKT_FLAG_KEY`。
const AV_PKT_FLAG_KEY: i32 = 0x0001;

/// 硬件编码会话数上限（§3.6 `E6002`）。
///
/// 消费级 NVIDIA 驱动的 NVENC 并发会话上限通常是 8；这里取同样的保守值。
pub const MAX_HARDWARE_SESSIONS: usize = 8;

/// 当前进程内已建立的硬件编码会话数。
static ACTIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// 硬件编码会话计数守卫。
///
/// 建立会话时计数 +1，释放时 -1；超过 [`MAX_HARDWARE_SESSIONS`] 返回 `E6002`。
#[derive(Debug)]
pub struct SessionGuard {
    _private: (),
}

impl SessionGuard {
    /// 申请一个硬件编码会话。
    pub fn acquire() -> Result<Self, EncError> {
        let prev = ACTIVE_SESSIONS.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_HARDWARE_SESSIONS {
            ACTIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
            return Err(EncError::SessionLimit);
        }
        Ok(Self { _private: () })
    }

    /// 当前进程内的活跃会话数（诊断用）。
    pub fn active() -> usize {
        ACTIVE_SESSIONS.load(Ordering::SeqCst)
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        ACTIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 编码器的输入格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    /// 直接吃 D3D11 硬件帧（§3.6「D3D11 硬件帧上下文」）。
    D3D11,
    /// 吃软件 NV12 帧（GPU 已完成颜色转换，这里只做一次回读）。
    Nv12,
}

impl InputMode {
    /// 对应的 ffmpeg 像素格式。
    fn pix_fmt(self) -> ffi::AVPixelFormat {
        match self {
            InputMode::D3D11 => ffi::AVPixelFormat::AV_PIX_FMT_D3D11,
            InputMode::Nv12 => ffi::AVPixelFormat::AV_PIX_FMT_NV12,
        }
    }
}

/// 已打开的原生编码上下文。
///
/// **字段顺序即析构顺序**（自定义 `Drop` 先跑，随后按声明顺序析构各字段）：
/// ffmpeg 侧的上下文（设备上下文 / 帧池）必须**先于**我们自己的 COM 引用释放，
/// 否则帧池里的 D3D11 纹理会在设备/设备上下文已经被释放之后才被 Release。
struct NativeEncoder {
    /// `AVCodecContext`。
    ctx: *mut ffi::AVCodecContext,
    /// ffmpeg 的 D3D11VA 设备上下文（保活：编码器上下文引用着它）。
    _hw: Option<HwDevice>,
    /// D3D11 硬件帧池。
    frames: Option<HwFrames>,
    /// GPU 颜色转换器。
    converter: Option<RgbToNv12>,
    /// 抓屏纹理所在的 D3D11 设备（与 ffmpeg 的硬件上下文同一个设备）。
    device: ID3D11Device,
    /// 设备立即上下文。
    context: ID3D11DeviceContext,
    /// 输入格式。
    input: InputMode,
    /// 因不被支持而被降档丢弃的参数。
    degraded: Vec<String>,
}

impl std::fmt::Debug for NativeEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeEncoder")
            .field("input", &self.input)
            .field("degraded", &self.degraded)
            .finish()
    }
}

impl Drop for NativeEncoder {
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

/// ffmpeg 后端的硬件视频编码器。
pub struct FfmpegVideoEncoder {
    /// 编码配置。
    cfg: EncodeConfig,
    /// 选中的厂商。
    kind: EncoderKind,
    /// ffmpeg 里的编码器名。
    name: &'static str,
    /// 原生上下文（延迟到第一帧建立，因为需要抓屏纹理所在的 D3D11 设备）。
    native: Option<NativeEncoder>,
    /// 是否已经请求过 IDR。
    idr_pending: bool,
    /// 递增的帧号。
    frame_id: u32,
    /// 已经产出、但还没交给调用方的码流。
    ///
    /// 硬件编码器有**一帧延迟**：送进第 N 帧时，第 N-1 帧的码流才吐出来，
    /// 所以码流先落在队列里，下一次 `encode()` 才交给调用方。
    pending: std::collections::VecDeque<EncodedFrame>,
    /// 硬件编码会话计数。
    _session: SessionGuard,
}

impl std::fmt::Debug for FfmpegVideoEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfmpegVideoEncoder")
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("cfg", &self.cfg)
            .field("idr_pending", &self.idr_pending)
            .finish()
    }
}

impl FfmpegVideoEncoder {
    /// 按配置建立编码器（此时还不碰原生句柄）。
    ///
    /// - 本机没有可用硬编 → `E6001`。
    /// - 硬件编码会话数超限 → `E6002`。
    /// - 配置的编解码器不被支持 → `E6003`。
    pub fn open(cfg: EncodeConfig) -> Result<Self, EncError> {
        let caps = crate::ffmpeg::probe::probe_encoders();
        let kind = select_encoder_kind(&caps, cfg.encoder).ok_or(EncError::NoHardwareEncoder)?;
        if !supports_codec(cfg.codec) {
            return Err(EncError::UnsupportedParam {
                what: format!("编解码器 {:?} 在本次构建中不可用", cfg.codec),
            });
        }
        let name = params::encoder_name(kind, cfg.codec).ok_or(EncError::UnsupportedParam {
            what: format!("{kind:?} 不支持 {:?}", cfg.codec),
        })?;
        let session = SessionGuard::acquire()?;
        Ok(Self {
            cfg,
            kind,
            name,
            native: None,
            idr_pending: false,
            frame_id: 0,
            pending: std::collections::VecDeque::new(),
            _session: session,
        })
    }

    /// 选中的厂商。
    pub fn kind(&self) -> EncoderKind {
        self.kind
    }

    /// ffmpeg 里的编码器名。
    pub fn encoder_name(&self) -> &'static str {
        self.name
    }

    /// 因不被支持而被降档丢弃的参数（§3.6 `E6003`）。
    pub fn degraded_params(&self) -> &[String] {
        self.native
            .as_ref()
            .map(|n| n.degraded.as_slice())
            .unwrap_or(&[])
    }

    /// 请求下一个关键帧（IDR）。
    pub fn request_idr(&mut self) {
        self.idr_pending = true;
    }

    /// 码率热更新（不重建编码器）。
    #[allow(
        unsafe_code,
        reason = "需要直接写 AVCodecContext 的码率字段（ffmpeg FFI）"
    )]
    pub fn set_bitrate(&mut self, bps: u32) {
        self.cfg.bitrate_bps = bps;
        if let Some(native) = self.native.as_mut() {
            // 编码器已经打开：直接改上下文字段，ffmpeg 会在下一帧生效。
            unsafe {
                (*native.ctx).bit_rate = i64::from(bps);
                (*native.ctx).rc_max_rate = i64::from(bps);
            }
        }
    }

    /// 编码一帧。
    ///
    /// **一帧延迟**：硬件编码器把帧送进去之后不会立刻吐出码流，
    /// 所以本次调用通常返回的是**上一帧**的编码结果；
    /// 第一次调用时编码器还没吐出任何东西，返回
    /// [`EncError::NoOutputYet`]（`E6005`）——这是正常状态，不是故障。
    pub fn encode(&mut self, f: &CapturedFrame) -> Result<EncodedFrame, EncError> {
        if self.native.is_none() {
            self.open_native(f)?;
        }
        // 先把 native 取出，避免 self 与 self.native 同时可变借用。
        let mut native = self.native.take().expect("native 已建立");
        let result = self.encode_inner(&mut native, f);
        self.native = Some(native);
        result
    }

    /// 编码流程（`native` 已经从 `self` 里取出）。
    fn encode_inner(
        &mut self,
        native: &mut NativeEncoder,
        f: &CapturedFrame,
    ) -> Result<EncodedFrame, EncError> {
        // 1) GPU 颜色转换：RGB → NV12。
        if native.converter.is_none() {
            native.converter = Some(RgbToNv12::new(&f.tex)?);
        }
        let converter = native.converter.as_mut().expect("converter 已建立");
        let nv12 = converter.convert(&f.tex)?.clone();

        // 2) 组帧。
        let frame = self.build_frame(native, f, &nv12)?;

        // 3) 送帧 + 收包。
        let mut out: Option<EncodedFrame> = None;
        self.send_and_receive(native, frame, f, &mut out)?;

        self.frame_id = self.frame_id.wrapping_add(1);
        self.idr_pending = false;
        if let Some(o) = out {
            self.pending.push_back(o);
        }
        // 硬件编码器有一帧延迟：这次交出去的是上一帧的结果。
        self.pending.pop_front().ok_or(EncError::NoOutputYet)
    }

    /// 建立原生编码上下文。
    ///
    /// 延迟到第一帧才建立，是因为 D3D11 硬件设备上下文必须用**抓屏纹理所在的
    /// 那个 `ID3D11Device`** 来创建（§3.6 的「D3D11 硬件帧上下文」）。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的编码器查找 FFI 与 D3D11 的 COM 调用（GetDevice）"
    )]
    fn open_native(&mut self, f: &CapturedFrame) -> Result<(), EncError> {
        let codec = find_encoder(self.name);
        if codec.is_null() {
            return Err(EncError::NoHardwareEncoder);
        }

        let device = unsafe { f.tex.GetDevice() }.map_err(|_| EncError::NoHardwareEncoder)?;
        let context =
            unsafe { device.GetImmediateContext() }.map_err(|_| EncError::NoHardwareEncoder)?;
        let hw = HwDevice::from_d3d11_device(&device)?;

        let input = pick_input_mode(codec)?;
        let frames = if input == InputMode::D3D11 {
            Some(HwFrames::new(&hw, f.width, f.height)?)
        } else {
            None
        };

        let (ctx, degraded) = open_encoder(
            codec,
            self.kind,
            &self.cfg,
            input.pix_fmt(),
            Some(&hw),
            frames.as_ref(),
        )?;

        self.native = Some(NativeEncoder {
            ctx,
            _hw: Some(hw),
            frames,
            converter: None,
            device,
            context,
            input,
            degraded,
        });
        Ok(())
    }

    /// 组装送进编码器的 `AVFrame`。
    #[allow(
        unsafe_code,
        reason = "需要直接写 AVFrame 字段并调用 ffmpeg 的帧分配 FFI"
    )]
    fn build_frame(
        &mut self,
        native: &mut NativeEncoder,
        f: &CapturedFrame,
        nv12: &ID3D11Texture2D,
    ) -> Result<*mut ffi::AVFrame, EncError> {
        let frame = unsafe { ffi::av_frame_alloc() };
        if frame.is_null() {
            return Err(EncError::NoHardwareEncoder);
        }
        let mut frame = frame;
        unsafe {
            (*frame).width = f.width as i32;
            (*frame).height = f.height as i32;
            // 时基是 1/fps，pts 用抓屏时间戳换算。
            let fps = self.cfg.fps.max(1) as i64;
            (*frame).pts = f.ts_us as i64 * fps / 1_000_000;
            if self.idr_pending {
                // 强制 IDR：把帧类型标成 I。
                (*frame).pict_type = ffi::AVPictureType::AV_PICTURE_TYPE_I;
            }
        }

        match native.input {
            InputMode::D3D11 => {
                let frames = native.frames.as_ref().ok_or(EncError::UnsupportedParam {
                    what: "D3D11 帧池未建立".to_string(),
                })?;
                unsafe {
                    (*frame).format = ffi::AVPixelFormat::AV_PIX_FMT_D3D11 as i32;
                    let ret = ffi::av_hwframe_get_buffer(frames.as_ptr(), frame, 0);
                    if ret < 0 {
                        ffi::av_frame_free(&mut frame);
                        return Err(EncError::UnsupportedParam {
                            what: "无法从 ffmpeg 帧池取 D3D11 帧".to_string(),
                        });
                    }
                }
                // 把转换后的 NV12 拷进 ffmpeg 的帧（同设备，纹理互拷）。
                let dst = frame_texture(frame).ok_or(EncError::NoHardwareEncoder)?;
                unsafe { native.context.CopyResource(&dst, nv12) };
            }
            InputMode::Nv12 => {
                let planes = read_back_nv12(&native.device, &native.context, nv12)?;
                unsafe {
                    (*frame).format = ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
                    (*frame).linesize[0] = planes.linesize as i32;
                    (*frame).linesize[1] = planes.linesize as i32;
                    let ret = ffi::av_frame_get_buffer(frame, 32);
                    if ret < 0 {
                        ffi::av_frame_free(&mut frame);
                        return Err(EncError::UnsupportedParam {
                            what: "无法分配 NV12 帧缓冲".to_string(),
                        });
                    }
                    let y_rows = f.height as usize;
                    let uv_rows = (f.height as usize) / 2;
                    copy_plane(
                        planes.y.as_ptr(),
                        (*frame).data[0],
                        planes.linesize,
                        (*frame).linesize[0] as usize,
                        y_rows,
                    );
                    copy_plane(
                        planes.uv.as_ptr(),
                        (*frame).data[1],
                        planes.linesize,
                        (*frame).linesize[1] as usize,
                        uv_rows,
                    );
                }
            }
        }
        Ok(frame)
    }

    /// 送帧并取包。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的编解码 FFI 并读取 AVPacket 字段"
    )]
    fn send_and_receive(
        &mut self,
        native: &mut NativeEncoder,
        mut frame: *mut ffi::AVFrame,
        f: &CapturedFrame,
        out: &mut Option<EncodedFrame>,
    ) -> Result<(), EncError> {
        let ret = unsafe { ffi::avcodec_send_frame(native.ctx, frame) };
        unsafe { ffi::av_frame_free(&mut frame) };
        if ret < 0 {
            return Err(map_ffmpeg_error(ret, "送帧失败"));
        }
        self.drain_packets(native, f.ts_us, out)
    }

    /// 从编码器里取出已经产出的码流包。
    #[allow(
        unsafe_code,
        reason = "需要调用 ffmpeg 的编解码 FFI 并读取 AVPacket 字段"
    )]
    fn drain_packets(
        &mut self,
        native: &mut NativeEncoder,
        ts_us: u64,
        out: &mut Option<EncodedFrame>,
    ) -> Result<(), EncError> {
        let mut pkt = unsafe { ffi::av_packet_alloc() };
        if pkt.is_null() {
            return Err(EncError::NoHardwareEncoder);
        }
        loop {
            let ret = unsafe { ffi::avcodec_receive_packet(native.ctx, pkt) };
            if ret == ffi::AVERROR_EOF || ret == ffi::AVERROR(EAGAIN) {
                break;
            }
            if ret < 0 {
                unsafe { ffi::av_packet_free(&mut pkt) };
                return Err(map_ffmpeg_error(ret, "取包失败"));
            }
            let (data, size, is_key) = unsafe {
                let size = (*pkt).size as usize;
                let data = if size > 0 && !(*pkt).data.is_null() {
                    Bytes::copy_from_slice(std::slice::from_raw_parts((*pkt).data, size))
                } else {
                    Bytes::new()
                };
                let is_key = (*pkt).flags & AV_PKT_FLAG_KEY != 0;
                (data, size, is_key)
            };
            unsafe { ffi::av_packet_unref(pkt) };
            if size == 0 {
                continue;
            }
            let slices = params::split_into_slices(data, self.cfg.slices);
            *out = Some(EncodedFrame {
                frame_id: self.frame_id,
                pts_us: ts_us,
                capture_ts_us: ts_us,
                is_key,
                slices,
            });
        }
        unsafe { ffi::av_packet_free(&mut pkt) };
        Ok(())
    }

    /// 冲刷编码器，取出还压在编码器里的帧。
    ///
    /// 编码器可能缓存 0~1 帧；送入空帧（EOF）会让它把缓存的帧全部吐出来。
    /// 会话结束时调用一次，保证最后一帧不丢。
    #[allow(
        unsafe_code,
        reason = "需要向 ffmpeg 送入空帧（EOF）以冲刷编码器，这是 FFI 调用"
    )]
    pub fn flush(&mut self, capture_ts_us: u64) -> Option<EncodedFrame> {
        let mut native = self.native.take()?;
        let ret = unsafe { ffi::avcodec_send_frame(native.ctx, std::ptr::null()) };
        let mut out: Option<EncodedFrame> = None;
        if ret >= 0 {
            let _ = self.drain_packets(&mut native, capture_ts_us, &mut out);
        }
        self.native = Some(native);
        if let Some(o) = out {
            self.pending.push_back(o);
        }
        self.pending.pop_front()
    }
}

/// 把一个平面拷进 ffmpeg 分配的缓冲（按各自行距逐行拷贝）。
///
/// 两边的行距可能不同（回读缓冲是 `RowPitch`，ffmpeg 缓冲是 32 字节对齐），
/// 因此必须逐行拷贝，不能整块 `copy_nonoverlapping`。
#[allow(unsafe_code, reason = "需要向 ffmpeg 分配的缓冲里写像素数据")]
fn copy_plane(src: *const u8, dst: *mut u8, src_stride: usize, dst_stride: usize, rows: usize) {
    if src.is_null() || dst.is_null() {
        return;
    }
    let width = src_stride.min(dst_stride);
    for row in 0..rows {
        unsafe {
            std::ptr::copy_nonoverlapping(
                src.add(row * src_stride),
                dst.add(row * dst_stride),
                width,
            );
        }
    }
}

/// 从 D3D11 硬件帧里取出纹理（借用，不接管所有权）。
#[allow(unsafe_code, reason = "需要把 ffmpeg 帧里的裸指针借用成 COM 接口")]
fn frame_texture(frame: *mut ffi::AVFrame) -> Option<ID3D11Texture2D> {
    unsafe {
        let raw: *mut c_void = (*frame).data[0].cast::<c_void>();
        if raw.is_null() {
            return None;
        }
        ID3D11Texture2D::from_raw_borrowed(&raw).cloned()
    }
}

/// 按编码器支持的像素格式选择输入模式。
///
/// 诊断开关：设置环境变量 `BEAM_ENC_FORCE_NV12=1` 可强制走**软件 NV12** 路径
/// （回读内存后按 `AV_PIX_FMT_NV12` 送帧），用来在「D3D11 硬件帧路径」出问题时
/// 做 A/B 对照。默认按编码器能力自动选择。
#[allow(unsafe_code, reason = "需要读取 ffmpeg 的 AVCodec.pix_fmts 数组")]
fn pick_input_mode(codec: *const ffi::AVCodec) -> Result<InputMode, EncError> {
    if std::env::var_os("BEAM_ENC_FORCE_NV12").is_some() {
        return Ok(InputMode::Nv12);
    }
    if supports_pix_fmt(codec, ffi::AVPixelFormat::AV_PIX_FMT_D3D11) {
        return Ok(InputMode::D3D11);
    }
    if supports_pix_fmt(codec, ffi::AVPixelFormat::AV_PIX_FMT_NV12) {
        return Ok(InputMode::Nv12);
    }
    Err(EncError::UnsupportedParam {
        what: "编码器既不支持 D3D11 也不支持 NV12 输入".to_string(),
    })
}

/// 编码器是否支持某个像素格式。
#[allow(unsafe_code, reason = "需要遍历 ffmpeg 的 AVCodec.pix_fmts 数组")]
fn supports_pix_fmt(codec: *const ffi::AVCodec, fmt: ffi::AVPixelFormat) -> bool {
    unsafe {
        let list = (*codec).pix_fmts;
        if list.is_null() {
            return false;
        }
        let mut i = 0usize;
        loop {
            let cur = *list.add(i);
            if cur == ffi::AVPixelFormat::AV_PIX_FMT_NONE {
                return false;
            }
            if cur == fmt {
                return true;
            }
            i += 1;
        }
    }
}

/// 按名字查找 ffmpeg 编码器。
#[allow(unsafe_code, reason = "需要调用 ffmpeg 的编码器查找 FFI")]
pub fn find_encoder(name: &str) -> *const ffi::AVCodec {
    let cname = match CString::new(name) {
        Ok(v) => v,
        Err(_) => return std::ptr::null(),
    };
    unsafe { ffi::avcodec_find_encoder_by_name(cname.as_ptr()) }
}

/// 打开编码器，必要时按 §3.6 的降档顺序丢弃不被支持的参数后重试。
///
/// 返回 `(AVCodecContext, 被丢弃的参数)`。
#[allow(
    unsafe_code,
    reason = "需要调用 ffmpeg 的编码器打开 FFI 并管理 AVDictionary"
)]
pub fn open_encoder(
    codec: *const ffi::AVCodec,
    kind: EncoderKind,
    cfg: &EncodeConfig,
    pix_fmt: ffi::AVPixelFormat,
    hw: Option<&HwDevice>,
    frames: Option<&HwFrames>,
) -> Result<(*mut ffi::AVCodecContext, Vec<String>), EncError> {
    let mut options = params::baseline_options(kind, cfg);
    let mut dropped: Vec<String> = Vec::new();
    loop {
        match try_open(codec, cfg, pix_fmt, hw, frames, &options) {
            Ok(ctx) => return Ok((ctx, dropped)),
            Err(code) if code == ffi::AVERROR_OPTION_NOT_FOUND => {
                // §3.6 的 E6003：参数不被支持 → 降档。
                // ffmpeg 不告诉我们是哪个参数，按文档化的顺序逐个丢弃后重试。
                let victim = params::DROPPABLE_OPTIONS
                    .iter()
                    .find(|k| options.iter().any(|(kk, _)| kk == *k))
                    .copied();
                match victim {
                    Some(k) => {
                        options.retain(|(kk, _)| *kk != k);
                        dropped.push(k.to_string());
                    }
                    None => {
                        return Err(EncError::UnsupportedParam {
                            what: "编码器不支持 §3.6 参数基线中的关键项".to_string(),
                        })
                    }
                }
            }
            Err(code) => return Err(map_ffmpeg_error(code, "打开编码器失败")),
        }
    }
}

/// 真正执行一次 `avcodec_open2`。
#[allow(
    unsafe_code,
    reason = "需要调用 ffmpeg 的编码器打开 FFI 并管理 AVDictionary 生命周期"
)]
fn try_open(
    codec: *const ffi::AVCodec,
    cfg: &EncodeConfig,
    pix_fmt: ffi::AVPixelFormat,
    hw: Option<&HwDevice>,
    frames: Option<&HwFrames>,
    options: &[(&'static str, String)],
) -> Result<*mut ffi::AVCodecContext, i32> {
    let settings = params::context_settings(cfg);
    let mut ctx = unsafe { ffi::avcodec_alloc_context3(codec) };
    if ctx.is_null() {
        return Err(ffi::AVERROR(EINVAL));
    }
    unsafe {
        (*ctx).width = 1920;
        (*ctx).height = 1080;
        (*ctx).pix_fmt = pix_fmt;
        (*ctx).time_base = ffi::AVRational {
            num: settings.time_base.0,
            den: settings.time_base.1,
        };
        (*ctx).framerate = ffi::AVRational {
            num: settings.framerate.0,
            den: settings.framerate.1,
        };
        (*ctx).gop_size = settings.gop_size;
        (*ctx).max_b_frames = settings.max_b_frames;
        (*ctx).bit_rate = settings.bit_rate;
        (*ctx).rc_max_rate = settings.rc_max_rate;
        (*ctx).rc_buffer_size = settings.rc_buffer_size;
        if settings.low_delay {
            (*ctx).flags |= Flags::LOW_DELAY.bits() as i32;
        }
        if let Some(hw) = hw {
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(hw.as_ptr());
        }
        if let Some(frames) = frames {
            (*ctx).hw_frames_ctx = ffi::av_buffer_ref(frames.as_ptr());
        }
    }

    let mut dict: *mut ffi::AVDictionary = std::ptr::null_mut();
    for (key, value) in options {
        let ckey = CString::new(*key).unwrap_or_default();
        let cvalue = CString::new(value.as_str()).unwrap_or_default();
        unsafe {
            ffi::av_dict_set(&mut dict, ckey.as_ptr(), cvalue.as_ptr(), 0);
        }
    }
    let ret = unsafe { ffi::avcodec_open2(ctx, codec, &mut dict) };
    unsafe {
        ffi::av_dict_free(&mut dict);
    }
    if ret < 0 {
        unsafe {
            ffi::avcodec_free_context(&mut ctx);
        }
        return Err(ret);
    }
    Ok(ctx)
}

/// 把 ffmpeg 的错误码映射到 §3.6 的错误码。
fn map_ffmpeg_error(code: i32, ctx: &str) -> EncError {
    if code == ffi::AVERROR_OPTION_NOT_FOUND {
        return EncError::UnsupportedParam {
            what: format!("{ctx}: 编码器不支持该参数"),
        };
    }
    if code == ffi::AVERROR(EINVAL) {
        return EncError::UnsupportedParam {
            what: format!("{ctx}: 参数非法"),
        };
    }
    EncError::NoHardwareEncoder
}

/// 本次构建里可用的编解码器枚举（探测用）。
pub fn probe_codecs() -> Vec<CodecKind> {
    let mut out = vec![CodecKind::Hevc, CodecKind::H264];
    if supports_codec(CodecKind::Av1) {
        out.push(CodecKind::Av1);
    }
    out
}
