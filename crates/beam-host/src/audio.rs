//! 主机侧音频接线（§3.7）：WASAPI loopback 采集 → Opus 5 ms 帧 → 音频数据报。
//!
//! ## 音频走**独立路径**（不进 FEC 分组）
//!
//! §5 的帧头把冗余片标成 `type = 2`（FEC 冗余），接收端分不清它是
//! 「音频帧的冗余片」还是「视频帧的冗余片」——这是台账里登记的待裁决协议问题。
//! 因此本模块规定：
//!
//! - 一个音频数据报 = **一帧 Opus 负载**（5 ms @128 kbps ≈ 80 B），
//!   远小于 §5 的 1184 B 负载上限 ⇒ **不分片**；
//! - 音频数据报**不进 FEC 分组**（`fec_shards = 0`），
//!   **不带**视频路径那个 8 字节负载前缀；
//! - 字节级格式由 [`beam_audio::wire`] 统一实现（主机与客户端共用同一份）。
//!
//! 音频与画面走**同一条 QUIC 连接**（§5 三通道里的 datagram 通道），
//! 只是帧头的 `type` 不同（音频 = 1）。
//!
//! ## 音频模式（§3.7 `audio_mode=host+stream|stream_only|off`）
//!
//! | 模式 | 主机侧行为 |
//! |---|---|
//! | `host+stream` | 打开 WASAPI loopback 采集 → Opus 编码 → 发音频数据报 |
//! | `stream_only` | **不采集**主机系统声音（唯一可用的源就是 loopback），因此本实现不发音频帧；发送通道仍然就绪，调用方可自行喂帧 |
//! | `off` | 完全不接音频 |
//!
//! ## 失败即降级：音频出错**不会**打断画面
//!
//! 设备打不开（`E7001`）、Opus 编解码失败（`E7002`）、输出/发送失败
//! 都会**如实上报错误码**，但音频链路只是**停止**——串流继续，画面照旧。
//! 这正是 §3.7 「音频失败 ⇒ 降级为只有画面」的语义。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam_audio::wire;
use beam_audio::{AudioConfig, AudioError, AudioSource, OpusEncoder};
use beam_net::NetError;

/// 音频数据报发送端（§5：音频与画面走同一条 QUIC 连接的 datagram 通道）。
///
/// 抽成 trait 是为了**可测**：测试里用假发送器断言「发出去的数据报长什么样」，
/// 不用真的建 QUIC 连接。
pub trait AudioDatagramSender: Send + Sync {
    /// 发一条音频数据报。
    fn send_audio_datagram(&self, datagram: &[u8]) -> Result<(), NetError>;
}

/// 基于 QUIC 连接的音频发送端（与画面共用同一条连接）。
#[derive(Debug, Clone)]
pub struct NetAudioSender {
    conn: beam_net::Conn,
}

impl NetAudioSender {
    /// 组装发送端。
    pub fn new(conn: beam_net::Conn) -> Self {
        Self { conn }
    }

    /// 底层连接（与画面发送器是同一条）。
    pub fn conn(&self) -> &beam_net::Conn {
        &self.conn
    }
}

impl AudioDatagramSender for NetAudioSender {
    fn send_audio_datagram(&self, datagram: &[u8]) -> Result<(), NetError> {
        self.conn.send_datagram(datagram)
    }
}

/// 主机侧音频流水线的错误。
///
/// 错误码**按来源分段**：音频层故障落在 `E7xxx`，传输层故障落在 `E2xxx`。
#[derive(Debug, thiserror::Error)]
pub enum AudioPipelineError {
    /// 音频层错误（`E7xxx`）：设备打不开、Opus 编解码失败等。
    #[error("音频失败: {source}")]
    Audio {
        /// 底层音频错误。
        #[from]
        source: AudioError,
    },

    /// 传输层错误（`E2xxx`）：数据报发不出去。
    #[error("音频发送失败: {source}")]
    Transport {
        /// 底层网络错误。
        #[from]
        source: NetError,
    },
}

impl AudioPipelineError {
    /// 数字错误码（`E7xxx` / `E2xxx`）。
    pub fn code(&self) -> u16 {
        match self {
            AudioPipelineError::Audio { source } => source.code(),
            AudioPipelineError::Transport { source } => source.code(),
        }
    }

    /// `E` 前缀错误码字符串。
    pub fn code_str(&self) -> String {
        format!("E{}", self.code())
    }
}

/// 主机侧音频统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioStats {
    /// 已发出的音频帧数。
    pub frames_sent: u64,
    /// 出错次数。
    pub errors: u64,
    /// 最近一次错误的码位（`E` 段编号）。
    pub last_error_code: Option<u16>,
}

/// 音频统计的共享句柄（任务与调用方各持一份）。
#[derive(Debug, Clone, Default)]
pub struct AudioStatsHandle {
    inner: Arc<Mutex<AudioStats>>,
}

impl AudioStatsHandle {
    /// 当前统计快照。
    pub fn snapshot(&self) -> AudioStats {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 记一次「发出去一帧」。
    pub fn record_sent(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.frames_sent += 1;
    }

    /// 记一次错误（保留最近的码位）。
    pub fn record_error(&self, code: u16) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.errors += 1;
        guard.last_error_code = Some(code);
    }
}

/// 主机侧音频流水线：采集 → Opus 编码 → 音频数据报（§3.7）。
///
/// 三个部件都是泛型参数，便于测试注入假实现：
///
/// - `S`：[`beam_audio::AudioSource`]（真机是 `WasapiLoopbackSource`）；
/// - `E`：[`beam_audio::OpusEncoder`]（真机是 `OpusEncoderImpl`）；
/// - `T`：[`AudioDatagramSender`]（真机是 [`NetAudioSender`]）。
pub struct AudioPipeline<S, E, T> {
    source: S,
    encoder: E,
    sender: T,
    cfg: AudioConfig,
    stats: AudioStatsHandle,
    /// 音频帧序号（帧号由 [`beam_audio::wire::audio_frame_id`] 加上号段标记）。
    sequence: u32,
    started: bool,
}

impl<S, E, T> std::fmt::Debug for AudioPipeline<S, E, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPipeline")
            .field("cfg", &self.cfg)
            .field("stats", &self.stats.snapshot())
            .field("started", &self.started)
            .finish()
    }
}

impl<S, E, T> AudioPipeline<S, E, T>
where
    S: AudioSource,
    E: OpusEncoder,
    T: AudioDatagramSender,
{
    /// 组装流水线（统计句柄自建）。
    pub fn new(source: S, encoder: E, sender: T, cfg: AudioConfig) -> Self {
        Self::with_stats(source, encoder, sender, cfg, AudioStatsHandle::default())
    }

    /// 组装流水线并指定统计句柄。
    pub fn with_stats(
        source: S,
        encoder: E,
        sender: T,
        cfg: AudioConfig,
        stats: AudioStatsHandle,
    ) -> Self {
        Self {
            source,
            encoder,
            sender,
            cfg,
            stats,
            sequence: 0,
            started: false,
        }
    }

    /// 启动采集（打开设备）。设备打不开 ⇒ `E7001`。
    pub fn start(&mut self) -> Result<(), AudioPipelineError> {
        self.source.start(self.cfg.clone())?;
        self.started = true;
        Ok(())
    }

    /// 取一帧 PCM → Opus 编码 → 发一条音频数据报。
    ///
    /// - 返回 `Ok(false)`：这一轮**没有**新帧（采集侧还没攒够 5 ms），不是错误；
    /// - 返回 `Ok(true)`：发出了一条音频数据报；
    /// - 返回 `Err(...)`：编码失败（`E7002`）、负载超限（`E7005`）或发送失败（`E2xxx`）。
    pub fn poll(&mut self) -> Result<bool, AudioPipelineError> {
        let Some(pcm) = self.source.next_frame() else {
            return Ok(false);
        };
        self.encode_and_send(&pcm)?;
        Ok(true)
    }

    /// 把采集侧**攒下的所有帧**都编码并发出，返回发出的帧数。
    ///
    /// ## 为什么必须「一次发完」
    ///
    /// §3.7 的音频帧是 **5 ms 一帧** ⇒ 实时需要 **200 帧/秒**。
    /// 如果每轮循环只取一帧，消费速率就被**循环的唤醒间隔**限死了：
    /// 唤醒间隔一旦是 15 ms，速率就掉到 65 帧/秒，采集队列随即积压、
    /// 开始丢帧（实测：主机只送出 65 帧/秒，采集队列刷「积压」告警）。
    ///
    /// 一次把队列里攒下的帧全发走，速率就**与唤醒间隔无关**：
    /// 唤醒晚了，只是这一轮多发几帧。
    /// （实测对照：同样的采集 + Opus 编码 + 建数据报，在「一轮发完」的写法下
    /// 是 199.7 帧/秒 —— 处理能力本身完全够用，瓶颈在循环结构。）
    ///
    /// ## 空负载不发出
    ///
    /// Opus 编码器有**一帧的流水线延迟**（头几帧送进去还取不出包），
    /// 这时编码返回**空负载**。空负载**不是**一帧音频 ⇒ 不发数据报、
    /// 也不占用帧号（否则客户端会拿到一个空包，解码失败）。
    pub fn drain_available(&mut self) -> Result<usize, AudioPipelineError> {
        let mut sent = 0usize;
        while let Some(pcm) = self.source.next_frame() {
            if self.encode_and_send(&pcm)? {
                sent += 1;
            }
        }
        Ok(sent)
    }

    /// 编码一帧 PCM 并发出；返回是否真的发出了数据报。
    fn encode_and_send(&mut self, pcm: &beam_audio::PcmFrame) -> Result<bool, AudioPipelineError> {
        let opus = self.encoder.encode(pcm)?;
        if opus.is_empty() {
            // 编码器还在流水线里 ⇒ 这一帧还没编码出来，不是错误，也不发数据报。
            return Ok(false);
        }
        let datagram = wire::encode_audio_datagram(self.sequence, now_us(), &opus)?;
        self.sequence = self.sequence.wrapping_add(1);
        self.sender.send_audio_datagram(&datagram)?;
        self.stats.record_sent();
        Ok(true)
    }

    /// 停止采集。
    pub fn stop(&mut self) {
        self.source.stop();
        self.started = false;
    }

    /// 是否已启动采集。
    pub fn is_started(&self) -> bool {
        self.started
    }

    /// 统计句柄。
    pub fn stats(&self) -> &AudioStatsHandle {
        &self.stats
    }

    /// 当前配置。
    pub fn config(&self) -> &AudioConfig {
        &self.cfg
    }
}

/// 主机侧音频循环：开设备 → 编码 → 发数据报，直到出错或任务被取消。
///
/// 出错时**停掉采集**并把错误码如实抛给调用方（调用方负责记录并降级为「只有画面」）。
///
/// ## 每轮「一次发完」
///
/// 循环里调的是 [`AudioPipeline::drain_available`] 而不是 `poll()`：
/// 音频是 5 ms 一帧（实时 200 帧/秒），一轮只取一帧的话，速率会被
/// **循环的唤醒间隔**限死（实测 15 ms 的唤醒间隔 ⇒ 只有 65 帧/秒，
/// 采集队列一路积压到丢帧）。一轮把攒下的帧全发走，速率就与唤醒间隔无关。
pub async fn run_audio_loop<S, E, T>(
    source: S,
    encoder: E,
    sender: T,
    cfg: AudioConfig,
    stats: AudioStatsHandle,
) -> Result<(), AudioPipelineError>
where
    S: AudioSource + Send + 'static,
    E: OpusEncoder + Send + 'static,
    T: AudioDatagramSender + Send + 'static,
{
    let mut pipeline = AudioPipeline::with_stats(source, encoder, sender, cfg, stats);
    pipeline.start()?;
    loop {
        let sent = match pipeline.drain_available() {
            Ok(sent) => sent,
            Err(e) => {
                pipeline.stop();
                return Err(e);
            }
        };
        if sent > 0 {
            // 刚发过一批：立刻再问一次（队列里可能还有）。
            // 这里**不睡** —— 睡了就等于把速率交给唤醒间隔；
            // 但要 `yield_now` 让出调度，别占死一个 worker。
            tokio::task::yield_now().await;
            continue;
        }
        // 采集侧是非阻塞的：还没攒够 5 ms 时 `drain_available` 返回 0。
        // 这里睡 1 ms 再问一次，兼顾延迟（§3.7 采集→编码 ≤6 ms）与 CPU 占用。
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// 主机侧单调时钟读数（微秒）。
///
/// **与画面通道的 `capture_ts` 不同源**：画面用的是抓屏层自己的单调时钟，
/// 音频用的是这里的单调时钟，两者的零点不同 ⇒ 两个时间戳**不可互相比较**，
/// 也不能拿来算「音画同步偏差」。这里只是如实记录「这一帧音频是什么时候采到的」。
fn now_us() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use beam_audio::PcmFrame;
    use bytes::Bytes;

    /// 假采集源：按需吐出固定数量的 PCM 帧。
    struct FakeSource {
        frames: Vec<PcmFrame>,
        stopped: Arc<AtomicBool>,
    }

    impl FakeSource {
        fn new(frames: Vec<PcmFrame>) -> Self {
            Self {
                frames,
                stopped: Arc::new(AtomicBool::new(false)),
            }
        }

        /// 「采集已被停掉」的标志（用来断言 `stop()` 真的生效了）。
        fn stopped_flag(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.stopped)
        }
    }

    impl AudioSource for FakeSource {
        fn start(&mut self, _cfg: AudioConfig) -> Result<(), AudioError> {
            Ok(())
        }

        fn next_frame(&mut self) -> Option<PcmFrame> {
            if self.frames.is_empty() {
                None
            } else {
                Some(self.frames.remove(0))
            }
        }

        fn stop(&mut self) {
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    /// 假编码器：把 PCM 原样当成「Opus 负载」。
    struct FakeEncoder {
        fail: bool,
    }

    impl OpusEncoder for FakeEncoder {
        fn encode(&mut self, pcm: &PcmFrame) -> Result<Bytes, AudioError> {
            if self.fail {
                return Err(AudioError::CodecFailed {
                    reason: "假编码器故障".to_string(),
                });
            }
            Ok(pcm.samples.clone())
        }

        fn reset(&mut self) {}
    }

    /// 假发送器：把发出去的数据报攒起来。
    #[derive(Clone, Default)]
    struct FakeSender {
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
        count: Arc<AtomicUsize>,
    }

    impl AudioDatagramSender for FakeSender {
        fn send_audio_datagram(&self, datagram: &[u8]) -> Result<(), NetError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.sent.lock().unwrap().push(datagram.to_vec());
            Ok(())
        }
    }

    fn pcm_frame() -> PcmFrame {
        PcmFrame {
            samples: Bytes::from_static(b"fake-opus-bytes"),
            sample_rate: 48_000,
            channels: 2,
        }
    }

    fn cfg() -> AudioConfig {
        AudioConfig {
            mode: beam_audio::AudioMode::HostAndStream,
            ..AudioConfig::default()
        }
    }

    #[test]
    fn pipeline_sends_one_audio_datagram_per_pcm_frame() {
        let sender = FakeSender::default();
        let mut pipeline = AudioPipeline::new(
            FakeSource::new(vec![pcm_frame(), pcm_frame()]),
            FakeEncoder { fail: false },
            sender.clone(),
            cfg(),
        );
        pipeline.start().expect("应当能启动");

        assert!(pipeline.poll().expect("应当发得出去"), "第一帧应当发出去");
        assert!(pipeline.poll().expect("应当发得出去"), "第二帧应当发出去");
        assert!(
            !pipeline.poll().expect("没帧时不是错误"),
            "没有新帧时返回 Ok(false)"
        );

        let sent = sender.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sender.count.load(Ordering::SeqCst), 2);
        assert_eq!(pipeline.stats().snapshot().frames_sent, 2);
    }

    #[test]
    fn drain_available_sends_every_queued_frame_in_one_pass() {
        // §3.7：5 ms 一帧 ⇒ 实时 200 帧/秒。一轮只取一帧的话，
        // 速率会被循环的唤醒间隔限死 ⇒ 必须一次把攒下的帧全发走。
        let sender = FakeSender::default();
        let mut pipeline = AudioPipeline::new(
            FakeSource::new(vec![pcm_frame(), pcm_frame(), pcm_frame()]),
            FakeEncoder { fail: false },
            sender.clone(),
            cfg(),
        );
        pipeline.start().expect("应当能启动");

        assert_eq!(
            pipeline.drain_available().expect("应当发得出去"),
            3,
            "一轮必须把攒下的 3 帧全发走"
        );
        assert_eq!(sender.count.load(Ordering::SeqCst), 3);
        assert_eq!(pipeline.stats().snapshot().frames_sent, 3);

        // 队列空了 ⇒ 这一轮 0 帧。
        assert_eq!(pipeline.drain_available().expect("应当发得出去"), 0);
        assert_eq!(sender.count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn empty_opus_payload_is_not_sent_as_an_audio_datagram() {
        /// 假编码器：返回**空**负载（模拟编码器的一帧流水线延迟）。
        struct EmptyEncoder;
        impl OpusEncoder for EmptyEncoder {
            fn encode(&mut self, _pcm: &PcmFrame) -> Result<Bytes, AudioError> {
                Ok(Bytes::new())
            }
            fn reset(&mut self) {}
        }

        let sender = FakeSender::default();
        let mut pipeline = AudioPipeline::new(
            FakeSource::new(vec![pcm_frame()]),
            EmptyEncoder,
            sender.clone(),
            cfg(),
        );
        pipeline.start().expect("应当能启动");

        assert_eq!(
            pipeline.drain_available().expect("空负载不是错误"),
            0,
            "空负载不算发出去了一帧"
        );
        assert!(
            sender.sent.lock().unwrap().is_empty(),
            "空负载不得发出数据报（客户端会拿到空包）"
        );
        assert_eq!(pipeline.stats().snapshot().frames_sent, 0);
    }

    #[test]
    fn datagram_is_a_single_audio_frame_without_fec_or_prefix() {
        let sender = FakeSender::default();
        let mut pipeline = AudioPipeline::new(
            FakeSource::new(vec![pcm_frame()]),
            FakeEncoder { fail: false },
            sender.clone(),
            cfg(),
        );
        pipeline.start().expect("应当能启动");
        pipeline.poll().expect("应当发得出去");

        let sent = sender.sent.lock().unwrap();
        let dg = &sent[0];
        // 帧头 type = 1（音频），且不进 FEC 分组。
        assert_eq!(dg[0], 0x11, "ver=1 / type=1（音频）");
        assert_eq!(dg[8], 1, "data_shards = 1");
        assert_eq!(dg[9], 0, "fec_shards = 0：音频不进 FEC 分组");
        // 负载就是 Opus 字节，没有 8 字节前缀。
        assert_eq!(&dg[16..], b"fake-opus-bytes");
        assert_eq!(dg.len(), 16 + b"fake-opus-bytes".len());

        // 能被客户端侧的解析器解回来。
        let parsed = wire::decode_audio_datagram(dg).expect("应当能解析");
        assert_eq!(parsed.payload.as_ref(), b"fake-opus-bytes");
        assert_eq!(parsed.sequence(), 0);
    }

    #[test]
    fn codec_failure_surfaces_as_e7002_and_is_not_swallowed() {
        let sender = FakeSender::default();
        let mut pipeline = AudioPipeline::new(
            FakeSource::new(vec![pcm_frame()]),
            FakeEncoder { fail: true },
            sender.clone(),
            cfg(),
        );
        pipeline.start().expect("应当能启动");
        let err = pipeline.poll().expect_err("编码失败必须报错");
        assert_eq!(err.code(), 7002, "Opus 编解码失败必须是 E7002");
        assert!(
            sender.sent.lock().unwrap().is_empty(),
            "编码失败时不得发出任何数据报"
        );
    }

    #[test]
    fn source_start_failure_surfaces_as_e7001() {
        struct FailingSource;
        impl AudioSource for FailingSource {
            fn start(&mut self, _cfg: AudioConfig) -> Result<(), AudioError> {
                Err(AudioError::DeviceFailed {
                    reason: "没有音频设备".to_string(),
                })
            }
            fn next_frame(&mut self) -> Option<PcmFrame> {
                None
            }
            fn stop(&mut self) {}
        }

        let mut pipeline = AudioPipeline::new(
            FailingSource,
            FakeEncoder { fail: false },
            FakeSender::default(),
            cfg(),
        );
        let err = pipeline.start().expect_err("设备打不开必须报错");
        assert_eq!(err.code(), 7001, "设备打开失败必须是 E7001");
    }

    #[test]
    fn stop_closes_the_capture_source() {
        let source = FakeSource::new(vec![]);
        let stopped = source.stopped_flag();
        let mut pipeline = AudioPipeline::new(
            source,
            FakeEncoder { fail: false },
            FakeSender::default(),
            cfg(),
        );
        pipeline.start().expect("应当能启动");
        assert!(pipeline.is_started());
        pipeline.stop();
        assert!(!pipeline.is_started());
        assert!(stopped.load(Ordering::SeqCst), "stop 必须真的停掉采集源");
    }

    #[test]
    fn frame_ids_use_their_own_number_space() {
        let sender = FakeSender::default();
        let mut pipeline = AudioPipeline::new(
            FakeSource::new(vec![pcm_frame(), pcm_frame()]),
            FakeEncoder { fail: false },
            sender.clone(),
            cfg(),
        );
        pipeline.start().expect("应当能启动");
        pipeline.poll().expect("应当发得出去");
        pipeline.poll().expect("应当发得出去");

        let sent = sender.sent.lock().unwrap();
        let first = wire::decode_audio_datagram(&sent[0]).expect("应当能解析");
        let second = wire::decode_audio_datagram(&sent[1]).expect("应当能解析");
        assert_eq!(first.sequence(), 0);
        assert_eq!(second.sequence(), 1);
        assert!(
            first.frame_id & 0x8000_0000 != 0,
            "音频帧号必须在独立号段，不与画面帧号相撞"
        );
    }
}
