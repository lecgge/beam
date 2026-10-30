//! 客户端侧音频接线（§3.7）：音频数据报 → Opus 解码 → WASAPI 输出。
//!
//! ## 音频走**独立路径**（不进 FEC 分组）
//!
//! 主机侧发过来的音频数据报，帧头 `type = 1`（§5），**一帧 Opus 负载**，
//! 不分片、不带 FEC 冗余、不带视频路径那个 8 字节负载前缀。
//! 字节级格式由 [`beam_audio::wire`] 统一实现（主机与客户端共用同一份）。
//!
//! ## 失败即降级：音频出错**不会**打断画面
//!
//! Opus 解码失败（`E7002`）、输出失败（`E7003`）都**如实上报错误码**，
//! 但音频只是「这一帧没播出来」，画面照旧。

use std::sync::{Arc, Mutex};

use beam_audio::wire;
use beam_audio::{AudioError, AudioOutput, OpusDecoder, PcmFrame};

/// 音频数据报接收端（可替换，便于测试）。
///
/// 真机实现是 [`AudioReceiver`]（Opus 解码 → WASAPI 输出）；
/// 测试里可以换成假实现，只断言「收到什么、播了什么」。
pub trait AudioDatagramSink: Send {
    /// 处理一条音频数据报（解码 + 播放）。
    fn handle_datagram(&mut self, datagram: &[u8]) -> Result<(), AudioError>;
}

/// 按配置组装**默认真机**音频接收端（Opus 解码 → WASAPI 输出）。
///
/// ## 为什么放在 beam-client 而不是让调用方自己拼
///
/// §4 的分层规定：`beam-audio`（L1 平台层）只能被 host / client（L3）引用，
/// 桌面端（L4）**不能**直接依赖它 —— 而组装「Opus 解码器 + WASAPI 输出」
/// 需要的正是 `beam_audio` 的那两个类型。
///
/// 所以这条组装线放在 beam-client（L3）里，由它持有对 `beam-audio` 的依赖；
/// 上层（桌面端）只拿到一个 `Box<dyn AudioDatagramSink>`，不必知道底下是
/// 哪个解码器、哪个输出设备。
///
/// `audio_mode = off` 或 Opus 后端没启用时返回 `None` —— 调用方据此
/// **只关音频**，画面照旧（§3.7 的降级语义）。
pub fn default_audio_sink(cfg: beam_audio::AudioConfig) -> Option<Box<dyn AudioDatagramSink>> {
    if cfg.mode == beam_audio::AudioMode::Off {
        return None;
    }
    // 解码器打不开（Opus 后端未启用 ⇒ E7004）或输出设备打不开（E7001/E7003）
    // ⇒ **没有音频通路**。返回 None 让调用方「只关音频」，画面照旧（§3.7 降级语义）；
    // 原因写进日志，不静默。
    let decoder = match beam_audio::OpusDecoderImpl::new(cfg.clone()) {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(
                code = %err.code_str(),
                "Opus 解码器不可用,本次会话只走画面"
            );
            return None;
        }
    };
    let output = match beam_audio::WasapiAudioOutput::new(cfg) {
        Ok(o) => o,
        Err(err) => {
            tracing::warn!(
                code = %err.code_str(),
                "音频输出设备不可用,本次会话只走画面"
            );
            return None;
        }
    };
    Some(Box::new(AudioReceiver::new(decoder, output)))
}

/// 客户端侧音频统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioReceiveStats {
    /// 已解码并送进输出的帧数。
    pub frames_decoded: u64,
    /// 出错次数。
    pub errors: u64,
    /// 最近一次错误的码位（`E` 段编号）。
    pub last_error_code: Option<u16>,
}

/// 音频统计的共享句柄（传输被装箱后仍能读到）。
#[derive(Debug, Clone, Default)]
pub struct AudioReceiveStatsHandle {
    inner: Arc<Mutex<AudioReceiveStats>>,
}

impl AudioReceiveStatsHandle {
    /// 当前统计快照。
    pub fn snapshot(&self) -> AudioReceiveStats {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 记一次「解码并播出去了」。
    pub fn record_decoded(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.frames_decoded += 1;
    }

    /// 记一次错误（保留最近的码位）。
    pub fn record_error(&self, code: u16) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.errors += 1;
        guard.last_error_code = Some(code);
    }
}

/// 客户端侧音频接收端：音频数据报 → Opus 解码 → 输出。
///
/// - `D`：[`beam_audio::OpusDecoder`]（真机是 `OpusDecoderImpl`）；
/// - `O`：[`beam_audio::AudioOutput`]（真机是 `WasapiAudioOutput`）。
pub struct AudioReceiver<D, O> {
    decoder: D,
    output: O,
    stats: AudioReceiveStatsHandle,
}

impl<D, O> std::fmt::Debug for AudioReceiver<D, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioReceiver")
            .field("stats", &self.stats.snapshot())
            .finish()
    }
}

impl<D, O> AudioReceiver<D, O>
where
    D: OpusDecoder,
    O: AudioOutput,
{
    /// 组装接收端（统计句柄自建）。
    pub fn new(decoder: D, output: O) -> Self {
        Self::with_stats(decoder, output, AudioReceiveStatsHandle::default())
    }

    /// 组装接收端并指定统计句柄。
    pub fn with_stats(decoder: D, output: O, stats: AudioReceiveStatsHandle) -> Self {
        Self {
            decoder,
            output,
            stats,
        }
    }

    /// 统计句柄。
    pub fn stats(&self) -> &AudioReceiveStatsHandle {
        &self.stats
    }

    /// 底层解码器。
    pub fn decoder(&self) -> &D {
        &self.decoder
    }

    /// 底层输出。
    pub fn output(&self) -> &O {
        &self.output
    }

    /// 解码一帧 Opus 并送进输出。
    fn decode_and_play(&mut self, opus: &[u8]) -> Result<(), AudioError> {
        let pcm: PcmFrame = self.decoder.decode(opus)?;
        self.output.play(&pcm)?;
        self.stats.record_decoded();
        Ok(())
    }
}

impl<D, O> AudioDatagramSink for AudioReceiver<D, O>
where
    D: OpusDecoder,
    O: AudioOutput,
{
    fn handle_datagram(&mut self, datagram: &[u8]) -> Result<(), AudioError> {
        let parsed = wire::decode_audio_datagram(datagram)?;
        self.decode_and_play(&parsed.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use beam_audio::AudioConfig;
    use bytes::Bytes;

    /// 假解码器：把 Opus 负载原样当成 PCM。
    struct FakeDecoder {
        fail: bool,
    }

    impl OpusDecoder for FakeDecoder {
        fn decode(&mut self, packet: &[u8]) -> Result<PcmFrame, AudioError> {
            if self.fail {
                return Err(AudioError::CodecFailed {
                    reason: "假解码器故障".to_string(),
                });
            }
            Ok(PcmFrame {
                samples: Bytes::copy_from_slice(packet),
                sample_rate: 48_000,
                channels: 2,
            })
        }

        fn reset(&mut self) {}
    }

    /// 假输出：记录播出去的内容。
    #[derive(Clone, Default)]
    struct FakeOutput {
        played: Arc<Mutex<Vec<Vec<u8>>>>,
        count: Arc<AtomicUsize>,
        fail: bool,
    }

    impl AudioOutput for FakeOutput {
        fn play(&mut self, pcm: &PcmFrame) -> Result<(), AudioError> {
            if self.fail {
                return Err(AudioError::OutputFailed {
                    reason: "假输出故障".to_string(),
                });
            }
            self.count.fetch_add(1, Ordering::SeqCst);
            self.played.lock().unwrap().push(pcm.samples.to_vec());
            Ok(())
        }
    }

    fn audio_datagram(opus: &[u8]) -> Vec<u8> {
        wire::encode_audio_datagram(0, 1_000, opus).expect("编码应当成功")
    }

    #[test]
    fn datagram_is_decoded_and_played() {
        let output = FakeOutput::default();
        let mut receiver = AudioReceiver::new(FakeDecoder { fail: false }, output.clone());
        receiver
            .handle_datagram(&audio_datagram(b"opus-frame"))
            .expect("应当能解码并播放");
        assert_eq!(output.count.load(Ordering::SeqCst), 1);
        assert_eq!(
            output.played.lock().unwrap()[0].as_slice(),
            b"opus-frame",
            "播出去的必须是解码后的 PCM"
        );
        assert_eq!(receiver.stats().snapshot().frames_decoded, 1);
    }

    #[test]
    fn decoder_failure_surfaces_as_e7002() {
        let output = FakeOutput::default();
        let mut receiver = AudioReceiver::new(FakeDecoder { fail: true }, output.clone());
        let err = receiver
            .handle_datagram(&audio_datagram(b"opus-frame"))
            .expect_err("解码失败必须报错");
        assert_eq!(err.code(), 7002);
        assert_eq!(output.count.load(Ordering::SeqCst), 0, "解码失败不得播放");
    }

    #[test]
    fn output_failure_surfaces_as_e7003() {
        let output = FakeOutput {
            fail: true,
            ..FakeOutput::default()
        };
        let mut receiver = AudioReceiver::new(FakeDecoder { fail: false }, output);
        let err = receiver
            .handle_datagram(&audio_datagram(b"opus-frame"))
            .expect_err("输出失败必须报错");
        assert_eq!(err.code(), 7003);
    }

    #[test]
    fn malformed_datagram_surfaces_as_e7005() {
        let mut receiver = AudioReceiver::new(FakeDecoder { fail: false }, FakeOutput::default());
        let err = receiver
            .handle_datagram(&[0x11, 0x00])
            .expect_err("长度不足必须报错");
        assert_eq!(err.code(), 7005);
    }

    #[test]
    fn a_video_datagram_is_not_accepted_as_audio() {
        let mut dg = vec![0u8; 16];
        dg[0] = 0x10; // ver=1 / type=0（视频）
        let mut receiver = AudioReceiver::new(FakeDecoder { fail: false }, FakeOutput::default());
        let err = receiver
            .handle_datagram(&dg)
            .expect_err("画面数据报必须被拒绝");
        assert_eq!(err.code(), 7005);
    }

    #[test]
    fn config_default_keeps_audio_off() {
        // 客户端侧的默认配置不改变音频开关：开关由 `audio_mode` 决定（§3.7）。
        let cfg = AudioConfig::default();
        assert_eq!(cfg.mode, beam_audio::AudioMode::Off);
    }
}
