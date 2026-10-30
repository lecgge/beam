//! 带**音频分流**的会话传输（§5 三通道 + 音频 datagram）。
//!
//! ## 为什么需要它
//!
//! §5 里视频与音频走**同一条** QUIC 连接的 datagram 通道，靠帧头的 `type`
//! 区分（视频 = 0、音频 = 1、FEC 冗余 = 2）。
//!
//! 会话层下沉的 [`beam_session::NetTransport`] 用
//! [`beam_net::VideoStream`] 收画面，而 `VideoStream::try_next_frame` 会把
//! 连接上**到达的所有数据报**都读走并塞进**画面重组器** —— 音频数据报因此
//! 会被当成画面分片处理（帧号相撞时更糟：两边的分片会被拼进同一帧）。
//!
//! 所以「按 `type` 分流」必须发生在**唯一的数据报读点**上。
//! 本模块提供的就是这个读点：它自己读数据报，音频的交给
//! [`AudioDatagramSink`]，画面的交给画面重组器。
//!
//! ## 为什么不改 `beam-session`
//!
//! 分流逻辑本该住在会话层那份传输里（`crates/beam-session/src/net_transport.rs`），
//! 但那个文件不在本次改动的写入范围内。因此这里在 `beam-client` 侧提供
//! **一个可选的**传输实现：控制流 / 输入流 / 发帧**原样转发**给会话层那份
//! `NetTransport`（不重写分帧、不重写控制面），只有 **datagram 读点**换成
//! 自己的。重组与抖动缓冲用的仍是 `beam-net` 的同一套原语
//! （[`beam_net::Reassembler`] / [`beam_net::JitterBuffer`]），
//! 因此「帧重组语义」没有第二份实现。
//!
//! ## 用法
//!
//! ```ignore
//! let factory = DemuxTransportFactory::with_audio(addr, net_cfg, verifier, audio_cfg);
//! let transport = factory.connect()?;   // `TransportFactory` 的同步接口
//! ```
//!
//! 音频不可用（没有输出设备、Opus 后端未启用）时**只关音频**，画面照旧。

use std::net::SocketAddr;
use std::sync::Arc;

use beam_audio::{AudioConfig, AudioError, OpusDecoderImpl, WasapiAudioOutput};
use beam_net::{JitterBuffer, Reassembler};
use beam_proto::EncodedFrame;
use beam_session::{ControlChannel, NetTransport, SessionError, Transport, TransportFactory};

use crate::audio::{AudioDatagramSink, AudioReceiveStatsHandle, AudioReceiver};

/// 带音频分流的会话传输。
///
/// - 控制流 / 输入流 / 发帧：**转发**给会话层的 [`NetTransport`]（不重写）；
/// - datagram：**自己读**，按帧头 `type` 分流 —— 音频 → 音频接收端，
///   其它 → 画面重组器。
pub struct DemuxTransport {
    /// 会话层那份传输（控制面 / 输入面 / 发帧）。
    inner: NetTransport,
    /// datagram 源（与 `inner` 是同一条连接）。
    conn: beam_net::Conn,
    /// 画面重组器（视频数据片 + FEC 冗余片）。
    reassembler: Reassembler,
    /// 画面抖动缓冲（§3.2 `jitter_frames`）。
    jitter: JitterBuffer,
    /// 音频接收端（`None` = 本次会话不接音频）。
    audio: Option<Box<dyn AudioDatagramSink>>,
    /// 音频统计句柄（`None` = 本次会话不接音频）。
    audio_stats: Option<AudioReceiveStatsHandle>,
    /// 因超时被丢弃的画面帧数（§3.2 **E2004**）。
    dropped: u32,
}

impl std::fmt::Debug for DemuxTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemuxTransport")
            .field("has_audio", &self.audio.is_some())
            .field(
                "audio_stats",
                &self
                    .audio_stats
                    .as_ref()
                    .map(AudioReceiveStatsHandle::snapshot),
            )
            .field("dropped", &self.dropped)
            .finish()
    }
}

impl DemuxTransport {
    /// 组装传输。
    pub fn new(
        inner: NetTransport,
        conn: beam_net::Conn,
        audio: Option<Box<dyn AudioDatagramSink>>,
        audio_stats: Option<AudioReceiveStatsHandle>,
    ) -> Self {
        let jitter = JitterBuffer::new(conn.config().jitter_depth());
        Self {
            inner,
            conn,
            reassembler: Reassembler::new(),
            jitter,
            audio,
            audio_stats,
            dropped: 0,
        }
    }

    /// 音频统计句柄（`None` = 本次会话不接音频）。
    pub fn audio_stats(&self) -> Option<&AudioReceiveStatsHandle> {
        self.audio_stats.as_ref()
    }

    /// 处理一条音频数据报。
    ///
    /// 音频失败**只记录、不上抛**：画面必须照旧（§3.7 的降级语义）。
    fn handle_audio(&mut self, datagram: &[u8]) {
        if self.audio.is_none() {
            // 本次会话不接音频（`audio_mode=off` 或音频后端不可用）：
            // 数据报照收、照丢，但**不进画面重组器**，因此画面不受影响。
            tracing::trace!(
                bytes = datagram.len(),
                "客户端未启用音频,丢弃这条音频数据报"
            );
            return;
        }
        dispatch_audio(&mut self.audio, self.audio_stats.as_ref(), datagram);
    }
}

/// 把一条音频数据报交给音频接收端。
///
/// 音频失败**只记录、不上抛**：画面必须照旧（§3.7 的降级语义）。
fn dispatch_audio(
    audio: &mut Option<Box<dyn AudioDatagramSink>>,
    stats: Option<&AudioReceiveStatsHandle>,
    datagram: &[u8],
) {
    let Some(sink) = audio.as_mut() else {
        return;
    };
    if let Err(e) = sink.handle_datagram(datagram) {
        if let Some(stats) = stats {
            stats.record_error(e.code());
        }
        tracing::warn!(
            code = %e.code_str(),
            reason = %e,
            "音频数据报处理失败,降级为只有画面"
        );
    }
}

impl ControlChannel for DemuxTransport {
    fn send_control(&mut self, msg: beam_proto::ControlMsg) -> Result<(), SessionError> {
        self.inner.send_control(msg)
    }

    fn recv_control(&mut self) -> Option<Result<beam_proto::ControlMsg, SessionError>> {
        self.inner.recv_control()
    }

    fn flush(&mut self) -> Result<(), SessionError> {
        self.inner.flush()
    }
}

impl Transport for DemuxTransport {
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        self.inner.send_frame(f)
    }

    fn send_input(&mut self, ev: beam_proto::InputEvent) -> Result<(), SessionError> {
        self.inner.send_input(ev)
    }

    /// 接收下一帧画面（QUIC datagram，§5）。
    ///
    /// 语义与 [`beam_net::VideoStream::try_next_frame`] 一致（**不阻塞**）：
    ///
    /// - 读走连接上**所有**已到达的数据报；
    /// - 音频数据报（帧头 `type = 1`）走音频支路，**不进**画面重组器；
    /// - 其余数据报进画面重组器，完整帧先过抖动缓冲再交出去；
    /// - 重组超时的画面帧按 §3.2 记为 **E2004**（写日志 + 计数，不静默丢弃）。
    fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
        // 1) 清理超时的画面帧（E2004）。
        for frame_id in self.reassembler.expire_stale() {
            self.dropped += 1;
            tracing::warn!(frame_id, code = "E2004", "画面帧重组超时并被丢弃(E2004)");
        }

        // 2) 读走已经到达的数据报：音频分流，画面进重组器。
        loop {
            let datagram = match self.conn.try_read_datagram() {
                Ok(Some(dg)) => dg,
                Ok(None) => break,
                Err(e) => {
                    return Some(Err(SessionError::TransportFailed {
                        reason: format!("接收画面失败 {}: {e}", e.code_str()),
                    }))
                }
            };
            if beam_audio::wire::is_audio_datagram(&datagram) {
                self.handle_audio(&datagram);
                continue;
            }
            match self.reassembler.push_datagram(&datagram) {
                Ok(Some(frame)) => self.jitter.push(frame, beam_net::now_us()),
                Ok(None) => {}
                Err(e) => {
                    return Some(Err(SessionError::TransportFailed {
                        reason: format!("接收画面失败 {}: {e}", e.code_str()),
                    }))
                }
            }
        }

        // 3) 经抖动缓冲放行一帧。
        //
        // §5 的 16 字节帧头只有 `capture_ts` 一个时间戳字段，没有独立的 `pts`，
        // 因此接收端把 `pts_us` 取成 `capture_ts`（与 `NetTransport` 同口径）。
        match self.jitter.pop(beam_net::now_us()) {
            Some(frame) => Some(Ok(EncodedFrame {
                frame_id: frame.frame_id,
                pts_us: frame.pts_us,
                capture_ts_us: frame.capture_ts_us,
                is_key: frame.is_key,
                slices: vec![frame.payload],
            })),
            None => None,
        }
    }

    /// 取走因超时被丢弃的画面帧数（§3.2 **E2004**）。
    fn take_dropped_frames(&mut self) -> u32 {
        std::mem::take(&mut self.dropped)
    }
}

/// 传输工厂：建立 QUIC 连接，并（可选）接上音频。
#[derive(Debug)]
pub struct DemuxTransportFactory {
    addr: SocketAddr,
    cfg: beam_net::NetConfig,
    verifier: Arc<dyn beam_net::PeerVerifier>,
    /// 音频配置（`None` = 本次会话不接音频）。
    audio: Option<AudioConfig>,
    /// 音频统计（`connect()` 之后才有值）。
    audio_stats: Option<AudioReceiveStatsHandle>,
}

impl DemuxTransportFactory {
    /// 组装工厂（**不接**音频：只有画面）。
    pub fn new(
        addr: SocketAddr,
        cfg: beam_net::NetConfig,
        verifier: Arc<dyn beam_net::PeerVerifier>,
    ) -> Self {
        Self {
            addr,
            cfg,
            verifier,
            audio: None,
            audio_stats: None,
        }
    }

    /// 组装工厂并接上音频（§3.7）。
    pub fn with_audio(
        addr: SocketAddr,
        cfg: beam_net::NetConfig,
        verifier: Arc<dyn beam_net::PeerVerifier>,
        audio: AudioConfig,
    ) -> Self {
        Self {
            addr,
            cfg,
            verifier,
            audio: Some(audio),
            audio_stats: None,
        }
    }

    /// 音频统计句柄（`connect()` 之后才有值）。
    pub fn audio_stats(&self) -> Option<&AudioReceiveStatsHandle> {
        self.audio_stats.as_ref()
    }
}

impl TransportFactory for DemuxTransportFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
        let conn = futures::executor::block_on(async {
            beam_net::NetEndpoint::connect(self.addr, self.cfg.clone(), self.verifier.clone())
                .await
                .map_err(|e| SessionError::TransportFailed {
                    reason: format!("连接失败 {}: {e}", e.code_str()),
                })
        })?;
        // 控制流 / 输入流仍然由会话层那份传输负责（分帧格式只有一份实现）。
        let inner = futures::executor::block_on(NetTransport::connect(conn.clone()))?;

        // 音频（可选）：设备或 Opus 后端不可用时**只关音频**，画面照旧。
        let (audio, stats) = match self.audio.clone() {
            // §3.7 `audio_mode=off`：客户端明确不要音频。
            Some(cfg) if cfg.mode != beam_audio::AudioMode::Off => match build_audio_sink(cfg) {
                Ok((sink, handle)) => (Some(sink), Some(handle)),
                Err(e) => {
                    tracing::warn!(
                        code = %e.code_str(),
                        reason = %e,
                        "客户端音频不可用,降级为只有画面"
                    );
                    (None, None)
                }
            },
            _ => (None, None),
        };
        self.audio_stats = stats.clone();

        Ok(Box::new(DemuxTransport::new(inner, conn, audio, stats)))
    }
}

/// 组装真机音频接收端（Opus 解码 + WASAPI 输出）。
///
/// 任何一步失败都返回 `E7xxx`（设备打不开 `E7001`、后端未启用 `E7004`），
/// 由调用方降级为「只有画面」。
fn build_audio_sink(
    cfg: AudioConfig,
) -> Result<(Box<dyn AudioDatagramSink>, AudioReceiveStatsHandle), AudioError> {
    let decoder = OpusDecoderImpl::new(cfg.clone())?;
    let output = WasapiAudioOutput::new(cfg)?;
    let stats = AudioReceiveStatsHandle::default();
    let receiver = AudioReceiver::with_stats(decoder, output, stats.clone());
    Ok((Box::new(receiver), stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    use beam_audio::AudioError;

    /// 假音频接收端：把收到的数据报记到共享列表里。
    #[derive(Clone, Default)]
    struct RecordingAudio {
        received: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    }

    impl AudioDatagramSink for RecordingAudio {
        fn handle_datagram(&mut self, datagram: &[u8]) -> Result<(), AudioError> {
            self.received.lock().unwrap().push(datagram.to_vec());
            Ok(())
        }
    }

    /// 总是失败的音频接收端。
    struct FailingAudio;

    impl AudioDatagramSink for FailingAudio {
        fn handle_datagram(&mut self, _datagram: &[u8]) -> Result<(), AudioError> {
            Err(AudioError::CodecFailed {
                reason: "假故障".to_string(),
            })
        }
    }

    /// 测试用校验器：信任一切指纹。
    #[derive(Debug)]
    struct AllowAllVerifier;

    impl beam_net::PeerVerifier for AllowAllVerifier {
        fn verify(&self, _fp: &beam_pair::Fingerprint) -> bool {
            true
        }
    }

    #[test]
    fn audio_datagram_is_classified_by_the_frame_header() {
        let audio = beam_audio::wire::encode_audio_datagram(0, 0, b"opus").expect("应当能编码");
        assert!(beam_audio::wire::is_audio_datagram(&audio));
        // 画面帧头：ver=1 / type=0。
        let mut video = vec![0u8; 16];
        video[0] = 0x10;
        assert!(!beam_audio::wire::is_audio_datagram(&video));
    }

    #[test]
    fn dispatch_hands_the_datagram_to_the_audio_sink() {
        let recording = RecordingAudio::default();
        let received = recording.received.clone();
        let mut sink = Some(Box::new(recording) as Box<dyn AudioDatagramSink>);
        let dg = beam_audio::wire::encode_audio_datagram(0, 0, b"opus").expect("应当能编码");
        dispatch_audio(&mut sink, None, &dg);
        let got = received.lock().unwrap();
        assert_eq!(got.len(), 1, "音频数据报必须交给音频接收端");
        assert_eq!(got[0], dg, "交给接收端的必须是原始数据报");
    }

    #[test]
    fn dispatch_records_the_error_and_does_not_panic() {
        let mut sink = Some(Box::new(FailingAudio) as Box<dyn AudioDatagramSink>);
        let stats = AudioReceiveStatsHandle::default();
        let dg = beam_audio::wire::encode_audio_datagram(0, 0, b"opus").expect("应当能编码");
        // 音频失败**不得**影响画面：这里只记录，不上抛。
        dispatch_audio(&mut sink, Some(&stats), &dg);
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.errors, 1);
        assert_eq!(snapshot.last_error_code, Some(7002));
    }

    #[test]
    fn factory_without_audio_has_no_audio_stats() {
        let factory = DemuxTransportFactory::new(
            "127.0.0.1:1".parse().expect("合法回环地址"),
            beam_net::NetConfig::default(),
            Arc::new(AllowAllVerifier),
        );
        assert!(factory.audio_stats().is_none());
    }
}
