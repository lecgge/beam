//! WASAPI：主机侧 **loopback 采集** + 客户端侧 **输出**（Windows）。
//!
//! ## 分工
//!
//! - 这一层只做 **COM 调用与搬字节**；
//!   「格式解析 / 协商 / 转换 / 分帧」全在 [`crate::pcm`] 里（纯逻辑，可单测）。
//!
//! ## 采集侧：loopback 的三个关键点
//!
//! 1. **端点**：loopback 采的是「系统正在播放的声音」，所以要的是
//!    **render** 端点（`eRender`），不是 capture 端点（那是麦克风）。
//! 2. **标志**：`AUDCLNT_STREAMFLAGS_LOOPBACK` —— 让引擎把渲染流回灌给采集端。
//!    再加 `AUDCLNT_STREAMFLAGS_EVENTCALLBACK` —— 引擎用**事件**通知有新数据，
//!    而不是让调用方轮询。
//! 3. **分帧**：设备缓冲是**变长**的（10 ms / 20 ms 常见），
//!    而 §3.7 要求吐给编码器的是**固定 5 ms**（240 采样 @ 48 kHz），
//!    所以要**累积**（[`crate::pcm::FrameAssembler`]）到 240 个采样再吐一帧。
//!
//! ## 输出侧：格式协商
//!
//! 设备混音格式常常是 **f32**（甚至 44.1 kHz、5.1），而本 crate 的 PCM 是
//! **i16 交错 48 kHz**。处理顺序：
//!
//! 1. 先问设备 `IsFormatSupported`：能不能直接吃 i16 交错 48 kHz？
//!    能 ⇒ 原样写（零转换）。
//! 2. 不能 ⇒ 用设备混音格式，**自己转**（i16 → f32 / 重采样 / 声道重映射），
//!    转换逻辑在 [`crate::pcm::pcm_to_device`]。
//!
//! > ⚠️ **不能把 f32 当 i16 读**。那样会播出一段噪声还不报错 ——
//! > 所以这里显式区分 `SampleFormat::I16` 与 `SampleFormat::F32`，
//! > 并且**永远不猜**格式：解析不出来就报错（E7003），不「按 i16 读」。
//!
//! ## 线程模型
//!
//! - COM 套间是**线程局部**的，所以 COM 初始化与 COM 对象的使用都在
//!   **各自的工作线程**里做（采集一条、输出一条）。
//! - 输出侧：写缓冲在**专用线程**里做（§6「播放用专用 OS 线程」），
//!   `play()` 只负责投递 + 等回执，不长时间阻塞。

pub(crate) use imp::{LoopbackInner, OutputInner};

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{CloseHandle, RPC_E_CHANGED_MODE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
        WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    use crate::audio::{AudioConfig, PcmFrame};
    use crate::error::AudioError;
    use crate::pcm::{
        bytes_to_i16, device_to_pcm, pcm_to_device, plan_playback, DesiredPcm, FrameAssembler,
        MixFormat, PlaybackPlan, RawWaveFormat, WAVE_FORMAT_EXTENSIBLE, WAVE_FORMAT_PCM,
    };

    /// 设备缓冲写不进去时的等待上限（写线程内部用，不阻塞调用方太久）。
    const WRITE_DEADLINE: Duration = Duration::from_millis(100);
    /// `play()` 等写线程回执的上限。
    const PLAY_ACK_TIMEOUT: Duration = Duration::from_millis(300);
    /// 输出命令队列长度（帧）。
    const OUTPUT_QUEUE_LEN: usize = 8;
    /// 采集队列上限（帧）：200 帧 = 1 s。
    const CAPTURE_QUEUE_LEN: usize = 200;
    /// 等待引擎事件的超时（毫秒）：超时后回去看停止标志。
    const EVENT_WAIT_MS: u32 = 100;
    /// 工作线程启动/就绪的超时。
    const READY_TIMEOUT: Duration = Duration::from_secs(5);

    // -----------------------------------------------------------------------
    // COM
    // -----------------------------------------------------------------------

    /// COM 套间守卫。
    struct ComGuard {
        /// 是否需要在析构时 `CoUninitialize`。
        needs_uninit: bool,
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.needs_uninit {
                unsafe { CoUninitialize() };
            }
        }
    }

    /// 在**当前线程**初始化 COM。
    ///
    /// WASAPI 的 COM 对象要求「当前线程已进 COM」，所以这个函数必须在
    /// **用对象的那个线程**上调用（采集 / 输出各自的工作线程）。
    ///
    /// `RPC_E_CHANGED_MODE` **不是错误**：它表示本线程已经用别的套间
    /// （通常是单线程套间）初始化过 COM —— COM 已经就绪，继续用即可；
    /// 但此时**不能**再 `CoUninitialize`（那会把别人的套间引用计数减掉）。
    fn com_init() -> Result<ComGuard, AudioError> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.0 >= 0 {
            // S_OK / S_FALSE：成功（两种情况都增加了套间引用计数）。
            Ok(ComGuard { needs_uninit: true })
        } else if hr == RPC_E_CHANGED_MODE {
            tracing::debug!("COM 已是其它套间（RPC_E_CHANGED_MODE），继续使用");
            Ok(ComGuard {
                needs_uninit: false,
            })
        } else {
            Err(AudioError::DeviceFailed {
                reason: format!("CoInitializeEx 失败：{hr:?}"),
            })
        }
    }

    // -----------------------------------------------------------------------
    // 设备
    // -----------------------------------------------------------------------

    /// 打开默认**渲染**端点。
    ///
    /// loopback 采集要的是 render 端点（采「系统正在播放的声音」），
    /// 客户端输出也要写进 render 端点。
    fn default_render_device() -> Result<IMMDevice, AudioError> {
        let enumerator: IMMDeviceEnumerator = unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        }
        .map_err(|e| AudioError::DeviceFailed {
            reason: format!("枚举音频设备失败：{e:?}"),
        })?;
        unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(|e| {
            AudioError::DeviceFailed {
                reason: format!("取默认渲染端点失败：{e:?}"),
            }
        })
    }

    /// 设备混音格式（持有 `GetMixFormat` 分配的内存，析构时释放）。
    struct MixFormatHandle {
        /// 解析后的格式。
        format: MixFormat,
        /// `GetMixFormat` 返回的指针（`Initialize` 期间有效）。
        ptr: *mut WAVEFORMATEX,
    }

    impl Drop for MixFormatHandle {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(Some(self.ptr.cast_const().cast())) };
        }
    }

    /// 读设备混音格式。
    ///
    /// 失败返回**原因字符串**（不自己决定错误码）：采集侧算设备问题（E7001），
    /// 输出侧算格式协商问题（E7003）。
    fn read_mix_format(client: &IAudioClient) -> Result<MixFormatHandle, String> {
        let ptr =
            unsafe { client.GetMixFormat() }.map_err(|e| format!("取设备混音格式失败：{e:?}"))?;
        if ptr.is_null() {
            return Err("设备混音格式返回空指针".to_string());
        }
        let raw = unsafe { read_raw_format(ptr) };
        let format =
            MixFormat::from_raw(&raw).map_err(|reason| format!("设备混音格式不支持：{reason}"))?;
        Ok(MixFormatHandle { format, ptr })
    }

    /// 把 `WAVEFORMATEX`（或 `WAVEFORMATEXTENSIBLE`）的字段拷出来。
    ///
    /// `WAVEFORMATEX` 是 `packed(1)` 结构，**不能**对它内部的字段取引用
    /// （未对齐引用会被编译器拒绝），所以逐字段按值拷贝。
    unsafe fn read_raw_format(ptr: *const WAVEFORMATEX) -> RawWaveFormat {
        let wf = unsafe { &*ptr };
        let format_tag = wf.wFormatTag;
        let channels = wf.nChannels;
        let sample_rate = wf.nSamplesPerSec;
        let avg_bytes_per_sec = wf.nAvgBytesPerSec;
        let block_align = wf.nBlockAlign;
        let bits_per_sample = wf.wBitsPerSample;
        let cb_size = wf.cbSize;
        let sub_format = if format_tag == WAVE_FORMAT_EXTENSIBLE && cb_size >= 22 {
            // WAVEFORMATEXTENSIBLE = WAVEFORMATEX(18) + Samples(2) + ChannelMask(4) + SubFormat(16)
            let ext = unsafe { &*ptr.cast::<WAVEFORMATEXTENSIBLE>() };
            let guid = ext.SubFormat;
            Some(guid.to_u128())
        } else {
            None
        };
        RawWaveFormat {
            format_tag,
            channels,
            sample_rate,
            avg_bytes_per_sec,
            block_align,
            bits_per_sample,
            cb_size,
            sub_format,
        }
    }

    /// 构造 16 位整数 PCM 的 `WAVEFORMATEX`。
    fn wave_format_ex_i16(sample_rate: u32, channels: u16) -> WAVEFORMATEX {
        let block_align = channels * 2;
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: channels,
            nSamplesPerSec: sample_rate,
            nAvgBytesPerSec: sample_rate * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: 16,
            cbSize: 0,
        }
    }

    /// 设备是否**直接**支持目标 i16 格式。
    ///
    /// 注意：`IsFormatSupported` 返回 **S_OK(0)** 才算支持；
    /// S_FALSE(1) 的意思是「不支持，这是最接近的格式」。
    fn is_format_supported(client: &IAudioClient, desired: &DesiredPcm) -> bool {
        let wf = wave_format_ex_i16(desired.sample_rate, desired.channels);
        let hr = unsafe { client.IsFormatSupported(AUDCLNT_SHAREMODE_SHARED, &wf, None) };
        hr.0 == 0
    }

    // -----------------------------------------------------------------------
    // 采集侧
    // -----------------------------------------------------------------------

    /// 采集线程与调用方共享的状态。
    struct CaptureShared {
        /// 已经凑齐的 5 ms 帧（调用方 `next_frame` 取）。
        frames: Mutex<VecDeque<PcmFrame>>,
        /// 停止标志。
        stop: AtomicBool,
    }

    impl CaptureShared {
        /// 推入一帧；队列满了就**丢最旧的**（丢最新的会让时间轴倒退）。
        fn push_frame(&self, frame: PcmFrame) {
            let mut queue = self.frames.lock().unwrap_or_else(|e| e.into_inner());
            if queue.len() >= CAPTURE_QUEUE_LEN {
                queue.pop_front();
                tracing::warn!("采集队列积压，丢弃最旧的一帧");
            }
            queue.push_back(frame);
        }

        /// 取一帧；没有就返回 `None`。
        fn pop_frame(&self) -> Option<PcmFrame> {
            let mut queue = self.frames.lock().unwrap_or_else(|e| e.into_inner());
            queue.pop_front()
        }
    }

    /// 主机侧 WASAPI loopback 采集源。
    pub(crate) struct LoopbackInner {
        shared: Arc<CaptureShared>,
        worker: Option<JoinHandle<()>>,
    }

    impl LoopbackInner {
        /// 打开默认渲染端点的 loopback 采集。
        ///
        /// 设备打不开 ⇒ `E7001`（设备打开失败）。
        pub(crate) fn open(cfg: AudioConfig) -> Result<Self, AudioError> {
            let shared = Arc::new(CaptureShared {
                frames: Mutex::new(VecDeque::new()),
                stop: AtomicBool::new(false),
            });
            let (ready_tx, ready_rx) = sync_channel::<Result<(), AudioError>>(1);
            let worker_shared = Arc::clone(&shared);
            let worker = thread::Builder::new()
                .name("beam-audio-loopback".to_string())
                .spawn(move || {
                    if let Err(err) = capture_worker(worker_shared, cfg, &ready_tx) {
                        tracing::error!(error = %err, "loopback 采集线程退出");
                        // **启动阶段**失败 ⇒ 把真实原因回传给调用方
                        // （否则调用方只能看到「启动超时」，看不到真正原因）。
                        let _ = ready_tx.send(Err(err));
                    }
                })
                .map_err(|e| AudioError::DeviceFailed {
                    reason: format!("启动采集线程失败：{e}"),
                })?;
            match ready_rx.recv_timeout(READY_TIMEOUT) {
                Ok(Ok(())) => Ok(Self {
                    shared,
                    worker: Some(worker),
                }),
                Ok(Err(err)) => {
                    let _ = worker.join();
                    Err(err)
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = worker.join();
                    Err(AudioError::DeviceFailed {
                        reason: "采集线程启动超时".to_string(),
                    })
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = worker.join();
                    Err(AudioError::DeviceFailed {
                        reason: "采集线程意外退出".to_string(),
                    })
                }
            }
        }

        /// 取下一帧；暂无数据返回 `None`（**非阻塞**语义）。
        pub(crate) fn next_frame(&mut self) -> Option<PcmFrame> {
            self.shared.pop_frame()
        }

        /// 停止采集并回收线程（`Drop` 里做同样的事）。
        pub(crate) fn close(self) {
            drop(self);
        }
    }

    impl Drop for LoopbackInner {
        fn drop(&mut self) {
            self.shared.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// 采集线程主体。
    fn capture_worker(
        shared: Arc<CaptureShared>,
        cfg: AudioConfig,
        ready: &SyncSender<Result<(), AudioError>>,
    ) -> Result<(), AudioError> {
        // COM 必须在**本线程**初始化（COM 对象也在本线程用）。
        let _com = com_init()?;
        let device = default_render_device()?;
        let client: IAudioClient =
            unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|e| AudioError::DeviceFailed {
                reason: format!("激活音频客户端失败：{e:?}"),
            })?;
        let mix = read_mix_format(&client).map_err(|reason| AudioError::DeviceFailed { reason })?;
        // loopback 的关键：render 端点 + AUDCLNT_STREAMFLAGS_LOOPBACK，
        // 采的是「系统正在播放的声音」；EVENTCALLBACK 让引擎用事件通知有新数据。
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
        unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, mix.ptr, None) }
            .map_err(|e| AudioError::DeviceFailed {
                reason: format!("初始化 loopback 采集失败：{e:?}"),
            })?;
        let capture: IAudioCaptureClient =
            unsafe { client.GetService() }.map_err(|e| AudioError::DeviceFailed {
                reason: format!("取采集服务失败：{e:?}"),
            })?;
        let event = unsafe { CreateEventW(None, false, false, None) }.map_err(|e| {
            AudioError::DeviceFailed {
                reason: format!("创建采集事件失败：{e:?}"),
            }
        })?;
        if let Err(e) = unsafe { client.SetEventHandle(event) } {
            let _ = unsafe { CloseHandle(event) };
            return Err(AudioError::DeviceFailed {
                reason: format!("设置采集事件句柄失败：{e:?}"),
            });
        }
        if let Err(e) = unsafe { client.Start() } {
            let _ = unsafe { CloseHandle(event) };
            return Err(AudioError::DeviceFailed {
                reason: format!("启动采集失败：{e:?}"),
            });
        }
        let _ = ready.send(Ok(()));

        // ---- 采集循环 ----
        let mut assembler = FrameAssembler::with_default_frame(cfg.channels, cfg.sample_rate);
        let mut failed = false;
        loop {
            if shared.stop.load(Ordering::Relaxed) {
                break;
            }
            let waited = unsafe { WaitForSingleObject(event, EVENT_WAIT_MS) };
            if waited != WAIT_OBJECT_0 {
                // 超时：没有数据时引擎不会触发事件，回去看停止标志。
                continue;
            }
            loop {
                let packet = match unsafe { capture.GetNextPacketSize() } {
                    Ok(size) => size,
                    Err(e) => {
                        // 设备掉线（DEVICE_INVALIDATED 之类）时这里会一直失败，
                        // 记一条错误并结束采集，别空转。
                        tracing::error!(error = %e, "取采集包大小失败，结束采集");
                        failed = true;
                        break;
                    }
                };
                if packet == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut buffer_flags = 0u32;
                if let Err(e) = unsafe {
                    capture.GetBuffer(&mut data, &mut frames, &mut buffer_flags, None, None)
                } {
                    tracing::error!(error = %e, "取采集缓冲失败，结束采集");
                    failed = true;
                    break;
                }
                // 静音包的内容**未定义**，不能读 —— 自己造一份同样长度的静音。
                // 两条路径都走同一个转换函数：重采样是在**目标采样率**上做的，
                // 静音包如果不重采样，累积出来的帧数会和有声包对不上（时间轴漂移）。
                let packet_bytes = frames as usize * mix.format.frame_bytes();
                let bytes: Vec<u8> = if frames == 0 {
                    Vec::new()
                } else if buffer_flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 {
                    vec![0u8; packet_bytes]
                } else {
                    let raw = unsafe { std::slice::from_raw_parts(data, packet_bytes) };
                    raw.to_vec()
                };
                let _ = unsafe { capture.ReleaseBuffer(frames) };
                let samples = device_to_pcm(&bytes, &mix.format, cfg.sample_rate, cfg.channels);
                for frame in assembler.push(&samples) {
                    shared.push_frame(frame);
                }
            }
            if failed {
                break;
            }
        }
        let _ = unsafe { client.Stop() };
        let _ = unsafe { CloseHandle(event) };
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 输出侧
    // -----------------------------------------------------------------------

    /// 输出线程的命令。
    enum OutputCmd {
        /// 播放一帧 PCM。
        Play {
            /// 待播放的帧。
            pcm: PcmFrame,
            /// 回执通道（每条命令单独一个，避免超时后回执串台）。
            ack: SyncSender<Result<(), AudioError>>,
        },
    }

    /// 客户端侧 WASAPI 输出。
    pub(crate) struct OutputInner {
        /// 命令通道（`None` = 已关闭）。
        cmd_tx: Option<SyncSender<OutputCmd>>,
        worker: Option<JoinHandle<()>>,
    }

    impl OutputInner {
        /// 打开默认渲染端点。
        ///
        /// 设备打不开 ⇒ `E7001`；混音格式没法协商 ⇒ `E7003`。
        pub(crate) fn open(cfg: AudioConfig) -> Result<Self, AudioError> {
            let desired = DesiredPcm {
                sample_rate: cfg.sample_rate,
                channels: cfg.channels,
            };
            let (cmd_tx, cmd_rx) = sync_channel::<OutputCmd>(OUTPUT_QUEUE_LEN);
            let (ready_tx, ready_rx) = sync_channel::<Result<(), AudioError>>(1);
            let worker = thread::Builder::new()
                .name("beam-audio-output".to_string())
                .spawn(move || {
                    if let Err(err) = output_worker(cmd_rx, desired, &ready_tx) {
                        tracing::error!(error = %err, "WASAPI 输出线程退出");
                        // 启动阶段失败 ⇒ 把真实原因回传给调用方。
                        let _ = ready_tx.send(Err(err));
                    }
                })
                .map_err(|e| AudioError::DeviceFailed {
                    reason: format!("启动输出线程失败：{e}"),
                })?;
            match ready_rx.recv_timeout(READY_TIMEOUT) {
                Ok(Ok(())) => Ok(Self {
                    cmd_tx: Some(cmd_tx),
                    worker: Some(worker),
                }),
                Ok(Err(err)) => {
                    let _ = worker.join();
                    Err(err)
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = worker.join();
                    Err(AudioError::DeviceFailed {
                        reason: "输出线程启动超时".to_string(),
                    })
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = worker.join();
                    Err(AudioError::DeviceFailed {
                        reason: "输出线程意外退出".to_string(),
                    })
                }
            }
        }

        /// 播放一帧 PCM。
        ///
        /// 真正的写缓冲在**专用线程**里做（§6「播放用专用 OS 线程」）；
        /// 这里只投递命令并等回执，因此错误码（E7003）能如实回到调用方。
        pub(crate) fn play(&mut self, pcm: &PcmFrame) -> Result<(), AudioError> {
            let (ack_tx, ack_rx) = sync_channel::<Result<(), AudioError>>(1);
            let cmd_tx = self
                .cmd_tx
                .as_ref()
                .ok_or_else(|| AudioError::OutputFailed {
                    reason: "输出端已关闭".to_string(),
                })?;
            cmd_tx
                .try_send(OutputCmd::Play {
                    pcm: pcm.clone(),
                    ack: ack_tx,
                })
                .map_err(|e| match e {
                    TrySendError::Full(_) => AudioError::OutputFailed {
                        reason: "输出队列已满（设备跟不上）".to_string(),
                    },
                    TrySendError::Disconnected(_) => AudioError::OutputFailed {
                        reason: "输出线程已退出".to_string(),
                    },
                })?;
            match ack_rx.recv_timeout(PLAY_ACK_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err(AudioError::OutputFailed {
                    reason: "等待写缓冲回执超时".to_string(),
                }),
            }
        }

        /// 停止输出并回收线程（`Drop` 里做同样的事）。
        pub(crate) fn close(self) {
            drop(self);
        }
    }

    impl Drop for OutputInner {
        fn drop(&mut self) {
            // 先断开命令通道：写线程的 `recv` 会返回 `Disconnected` 并退出。
            // （不能只靠投 `Stop`：队列满的时候投不进去，join 就会永远等下去。）
            self.cmd_tx = None;
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// 输出线程主体。
    fn output_worker(
        cmd_rx: Receiver<OutputCmd>,
        desired: DesiredPcm,
        ready: &SyncSender<Result<(), AudioError>>,
    ) -> Result<(), AudioError> {
        let _com = com_init()?;
        let device = default_render_device()?;
        let client: IAudioClient =
            unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|e| AudioError::DeviceFailed {
                reason: format!("激活音频客户端失败：{e:?}"),
            })?;
        let mix = read_mix_format(&client).map_err(|reason| AudioError::OutputFailed { reason })?;

        // 先问设备「能不能直接吃 i16 交错 48 kHz」——
        // 能就零转换，不能就自己转（f32 / 其它采样率 / 其它声道数）。
        let native_supported = is_format_supported(&client, &desired);
        let plan = plan_playback(native_supported, &mix.format, &desired);
        tracing::debug!(?plan, "WASAPI 输出格式方案");

        // 初始化用的格式：Native / Convert 用混音格式，
        // AutoConvert 用我们想要的 i16 格式并打开 AUTOCONVERTPCM。
        let desired_wf = wave_format_ex_i16(desired.sample_rate, desired.channels);
        let (init_ptr, init_flags): (*const WAVEFORMATEX, u32) = match plan {
            PlaybackPlan::Native => (mix.ptr, 0),
            PlaybackPlan::AutoConvert => (
                &desired_wf as *const WAVEFORMATEX,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            ),
            PlaybackPlan::Convert(_) => (mix.ptr, 0),
        };
        unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, init_flags, 0, 0, init_ptr, None) }
            .map_err(|e| AudioError::DeviceFailed {
                reason: format!("初始化音频输出失败：{e:?}"),
            })?;
        let render: IAudioRenderClient =
            unsafe { client.GetService() }.map_err(|e| AudioError::DeviceFailed {
                reason: format!("取渲染服务失败：{e:?}"),
            })?;
        unsafe { client.Start() }.map_err(|e| AudioError::DeviceFailed {
            reason: format!("启动音频输出失败：{e:?}"),
        })?;
        let _ = ready.send(Ok(()));

        // ---- 写循环 ----
        loop {
            match cmd_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(OutputCmd::Play { pcm, ack }) => {
                    let result = write_pcm(&client, &render, plan, &desired, &pcm);
                    let _ = ack.send(result);
                }
                Err(RecvTimeoutError::Timeout) => continue,
                // 命令通道断开 ⇒ 调用方已经关掉了输出端，退出线程。
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = unsafe { client.Stop() };
        Ok(())
    }

    /// 写一帧 PCM 到设备缓冲。
    fn write_pcm(
        client: &IAudioClient,
        render: &IAudioRenderClient,
        plan: PlaybackPlan,
        desired: &DesiredPcm,
        pcm: &PcmFrame,
    ) -> Result<(), AudioError> {
        let (bytes, frame_bytes): (Vec<u8>, usize) = match plan {
            PlaybackPlan::Native | PlaybackPlan::AutoConvert => {
                // 设备吃的就是 i16 交错（desired），**原样写**，不做任何转换。
                if pcm.channels != desired.channels || pcm.sample_rate != desired.sample_rate {
                    return Err(AudioError::OutputFailed {
                        reason: format!(
                            "PCM 参数与设备格式不符：期望 {} Hz / {} 声道，实际 {} Hz / {} 声道",
                            desired.sample_rate, desired.channels, pcm.sample_rate, pcm.channels
                        ),
                    });
                }
                (pcm.samples.to_vec(), usize::from(desired.channels) * 2)
            }
            PlaybackPlan::Convert(mix) => {
                // 设备混音格式不是我们要的形态 ⇒ 自己转（i16 交错 → 设备格式）。
                let samples = bytes_to_i16(&pcm.samples);
                (
                    pcm_to_device(&samples, pcm.channels, pcm.sample_rate, &mix),
                    mix.frame_bytes(),
                )
            }
        };
        if bytes.is_empty() {
            return Ok(());
        }
        if frame_bytes == 0 || bytes.len() % frame_bytes != 0 {
            return Err(AudioError::OutputFailed {
                reason: format!(
                    "PCM 长度不是帧长的整数倍（{} 字节 / 每帧 {frame_bytes} 字节）",
                    bytes.len()
                ),
            });
        }
        let frames = (bytes.len() / frame_bytes) as u32;
        let buffer_size =
            unsafe { client.GetBufferSize() }.map_err(|e| AudioError::OutputFailed {
                reason: format!("取输出缓冲大小失败：{e:?}"),
            })?;
        let deadline = Instant::now() + WRITE_DEADLINE;

        // 1) 等设备缓冲腾出空间（有上限，不无限期占着调用线程）。
        loop {
            let padding =
                unsafe { client.GetCurrentPadding() }.map_err(|e| AudioError::OutputFailed {
                    reason: format!("取输出缓冲占用失败：{e:?}"),
                })?;
            if buffer_size.saturating_sub(padding) >= frames {
                break;
            }
            if Instant::now() >= deadline {
                return Err(AudioError::OutputFailed {
                    reason: format!(
                        "设备缓冲没有空间（缓冲 {buffer_size} 帧，已占 {padding} 帧，要写 {frames} 帧）"
                    ),
                });
            }
            thread::sleep(Duration::from_millis(1));
        }

        // 2) 取缓冲 → 拷贝 → 提交。GetBuffer 可能因为与引擎的竞态失败，重试到 deadline。
        let ptr = loop {
            match unsafe { render.GetBuffer(frames) } {
                Ok(ptr) => break ptr,
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(AudioError::OutputFailed {
                            reason: format!("取输出缓冲失败：{e:?}"),
                        });
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            }
        };
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
        unsafe { render.ReleaseBuffer(frames, 0) }.map_err(|e| AudioError::OutputFailed {
            reason: format!("提交输出缓冲失败：{e:?}"),
        })?;
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::audio::{AudioConfig, PcmFrame};
    use crate::error::AudioError;

    /// 非 Windows 平台上的采集占位。
    pub(crate) struct LoopbackInner;

    impl LoopbackInner {
        /// 非 Windows 没有 WASAPI ⇒ `E7004`（后端未启用）。
        pub(crate) fn open(_cfg: AudioConfig) -> Result<Self, AudioError> {
            Err(AudioError::BackendDisabled)
        }

        /// 没有采集源可用。
        pub(crate) fn next_frame(&mut self) -> Option<PcmFrame> {
            None
        }

        /// 无操作。
        pub(crate) fn close(self) {}
    }

    /// 非 Windows 平台上的输出占位。
    pub(crate) struct OutputInner;

    impl OutputInner {
        /// 非 Windows 没有 WASAPI ⇒ `E7004`（后端未启用）。
        pub(crate) fn open(_cfg: AudioConfig) -> Result<Self, AudioError> {
            Err(AudioError::BackendDisabled)
        }

        /// 没有输出端可用。
        pub(crate) fn play(&mut self, _pcm: &PcmFrame) -> Result<(), AudioError> {
            Err(AudioError::BackendDisabled)
        }

        /// 无操作。
        pub(crate) fn close(self) {}
    }
}
