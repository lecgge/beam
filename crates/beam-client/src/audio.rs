//! 客户端侧音频接线（§3.7）：音频数据报 → Opus 解码 → WASAPI 输出。
//!
//! ## 实现已经下沉到 `beam-audio`（L1）
//!
//! 这套组装（[`AudioDatagramSink`] / [`AudioReceiver`] / 统计句柄）是**纯音频域**的
//! 逻辑，不依赖会话层也不依赖传输层。
//!
//! 而 §4 规定 **L3 之间不得横向依赖**（`beam-host` 与 `beam-client` 同层）：
//! 主机侧的整链验证入口（`beam-host` 的 `client_loopback` 示例）与客户端侧
//! 都需要用到它，唯一不违反分层的位置就是 L1 的 `beam-audio` 本身 ——
//! [`beam_audio::wire`] 的模块文档记的就是同一条教训：两端各写一份，
//! 字节级实现必然会悄悄漂移。
//!
//! 所以本模块现在只做**重导出**：对外的公开路径
//! （`beam_client::AudioReceiver` 等）保持不变，桌面端（L4）不受影响。

pub use beam_audio::receive::{
    default_audio_sink, AudioDatagramSink, AudioReceiveStats, AudioReceiveStatsHandle,
    AudioReceiver,
};

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use beam_audio::{AudioError, AudioOutput, OpusDecoder, PcmFrame};
    use bytes::Bytes;

    /// 假解码器：把 Opus 负载原样当成 PCM。
    struct FakeDecoder;

    impl OpusDecoder for FakeDecoder {
        fn decode(&mut self, packet: &[u8]) -> Result<PcmFrame, AudioError> {
            Ok(PcmFrame {
                samples: Bytes::copy_from_slice(packet),
                sample_rate: 48_000,
                channels: 2,
            })
        }

        fn reset(&mut self) {}
    }

    /// 假输出：只数次数。
    #[derive(Clone, Default)]
    struct FakeOutput {
        count: Arc<AtomicUsize>,
    }

    impl AudioOutput for FakeOutput {
        fn play(&mut self, _pcm: &PcmFrame) -> Result<(), AudioError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn reexported_receiver_still_decodes_and_plays() {
        let output = FakeOutput::default();
        let mut receiver = AudioReceiver::new(FakeDecoder, output.clone());
        let dg =
            beam_audio::wire::encode_audio_datagram(0, 1_000, b"opus-frame").expect("应当能编码");
        receiver.handle_datagram(&dg).expect("应当能解码并播放");
        assert_eq!(output.count.load(Ordering::SeqCst), 1);
        assert_eq!(receiver.stats().snapshot().frames_decoded, 1);
    }
}
