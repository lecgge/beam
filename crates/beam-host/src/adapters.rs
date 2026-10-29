//! 平台实现 → 会话 trait 的适配器（§3.9「通过 trait 注入，便于测试」）。

use beam_capture::{CaptureConfig, CaptureSource, CapturedFrame as CaptureCapturedFrame};
use beam_encode::{CapturedFrame as EncodeCapturedFrame, VideoEncoder};
use beam_input::InputSink as PlatformInputSink;
use beam_net::VideoSender as NetVideoSenderTrait;
use beam_proto::{ControlMsg, EncodedFrame, InputEvent};
use beam_session::{FramePipeline, InputSink, SessionError, VideoSender};

/// 抓屏 → 编码流水线适配器。
///
/// §4 规定 beam-encode 不能依赖 beam-capture，因此两边的 `CapturedFrame`
/// 是同形但独立的类型，这里做一次字段搬运。
#[derive(Debug)]
pub struct CaptureEncodePipeline<C, E>
where
    C: CaptureSource,
    E: VideoEncoder,
{
    capture: C,
    encoder: E,
    capture_cfg: CaptureConfig,
}

impl<C, E> CaptureEncodePipeline<C, E>
where
    C: CaptureSource,
    E: VideoEncoder,
{
    /// 组装适配器。
    pub fn new(capture: C, encoder: E, capture_cfg: CaptureConfig) -> Self {
        Self {
            capture,
            encoder,
            capture_cfg,
        }
    }

    /// 访问底层抓屏源。
    pub fn capture(&self) -> &C {
        &self.capture
    }

    /// 访问底层编码器。
    pub fn encoder(&self) -> &E {
        &self.encoder
    }
}

impl<C, E> FramePipeline for CaptureEncodePipeline<C, E>
where
    C: CaptureSource + Send,
    E: VideoEncoder + Send,
{
    fn start(&mut self) -> Result<(), SessionError> {
        self.capture
            .start(self.capture_cfg.clone())
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("抓屏启动失败 {}: {e}", e.code_str()),
            })
    }

    fn poll(&mut self) -> Option<EncodedFrame> {
        let frame = self.capture.latest()?;
        let (width, height) = texture_size(&frame);
        let input = EncodeCapturedFrame {
            tex: frame.tex,
            ts_us: frame.ts_us,
            dirty: frame.dirty,
            width,
            height,
        };
        self.encoder.encode(&input).ok()
    }

    fn request_idr(&mut self) {
        self.encoder.request_idr();
    }

    fn stop(&mut self) {
        self.capture.stop();
    }
}

#[cfg(windows)]
fn texture_size(frame: &CaptureCapturedFrame) -> (u32, u32) {
    // 真实实现从 D3D11 资源描述里取宽高。
    let _ = frame;
    (1920, 1080)
}

#[cfg(not(windows))]
fn texture_size(frame: &CaptureCapturedFrame) -> (u32, u32) {
    let _ = frame;
    (1920, 1080)
}

/// 网络发送适配器。
#[derive(Debug)]
pub struct NetVideoSender<S> {
    sender: S,
}

impl<S> NetVideoSender<S> {
    /// 组装适配器。
    pub fn new(sender: S) -> Self {
        Self { sender }
    }

    /// 访问底层发送器。
    pub fn inner(&self) -> &S {
        &self.sender
    }
}

impl<S> VideoSender for NetVideoSender<S>
where
    S: NetVideoSenderTrait + Send,
{
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        NetVideoSenderTrait::send_frame(&self.sender, f).map_err(|e| {
            SessionError::TransportFailed {
                reason: format!("发送画面失败 {}: {e}", e.code_str()),
            }
        })
    }

    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        // 控制消息由 QUIC 可靠流承载，这里只做适配占位。
        let _ = msg;
        Ok(())
    }
}

/// 输入注入适配器。
#[derive(Debug)]
pub struct InputAdapter<S> {
    sink: S,
}

impl<S> InputAdapter<S> {
    /// 组装适配器。
    pub fn new(sink: S) -> Self {
        Self { sink }
    }

    /// 访问底层注入器。
    pub fn inner(&self) -> &S {
        &self.sink
    }
}

impl<S> InputSink for InputAdapter<S>
where
    S: PlatformInputSink + Send,
{
    fn apply(&mut self, ev: InputEvent) -> Result<(), SessionError> {
        PlatformInputSink::apply(&mut self.sink, ev).map_err(|e| SessionError::TransportFailed {
            reason: format!("注入输入失败 {}: {e}", e.code_str()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_capture::{CapError, CaptureBackend};
    use beam_input::{InputError, SendInputSink};
    use beam_proto::{KeyAction, StatsReport};
    use bytes::Bytes;

    struct FakeCapture {
        frames: Vec<Option<CaptureCapturedFrame>>,
    }

    impl CaptureSource for FakeCapture {
        fn start(&mut self, _cfg: CaptureConfig) -> Result<(), CapError> {
            Ok(())
        }
        fn latest(&mut self) -> Option<CaptureCapturedFrame> {
            if self.frames.is_empty() {
                None
            } else {
                self.frames.remove(0)
            }
        }
        fn stop(&mut self) {}
    }

    struct FakeEncoder {
        idr: bool,
    }

    impl VideoEncoder for FakeEncoder {
        fn encode(
            &mut self,
            f: &EncodeCapturedFrame,
        ) -> Result<EncodedFrame, beam_encode::EncError> {
            Ok(EncodedFrame {
                frame_id: f.ts_us as u32,
                pts_us: f.ts_us,
                capture_ts_us: f.ts_us,
                is_key: true,
                slices: vec![Bytes::from_static(b"x")],
            })
        }
        fn request_idr(&mut self) {
            self.idr = true;
        }
        fn set_bitrate(&mut self, _bps: u32) {}
    }

    struct FakeNetSender {
        sent: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl NetVideoSenderTrait for FakeNetSender {
        fn send_frame(&self, _f: EncodedFrame) -> Result<(), beam_net::NetError> {
            self.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn pipeline_adapts_capture_to_encoder() {
        let capture = FakeCapture { frames: vec![] };
        let encoder = FakeEncoder { idr: false };
        let mut pipeline = CaptureEncodePipeline::new(
            capture,
            encoder,
            CaptureConfig {
                backend: CaptureBackend::Auto,
                display_id: None,
                min_update_interval: None,
                target_fps: 60,
            },
        );
        assert!(pipeline.start().is_ok());
        assert!(pipeline.poll().is_none());
        pipeline.request_idr();
        assert!(pipeline.encoder().idr);
    }

    #[test]
    fn input_adapter_maps_errors() {
        let mut adapter = InputAdapter::new(SendInputSink::new());
        let err = adapter
            .apply(InputEvent::Key {
                code: 4,
                action: KeyAction::Down,
            })
            .expect_err("无权限必须失败");
        assert_eq!(err.code(), 9005);
        let _ = StatsReport::default();
    }

    #[test]
    fn net_sender_adapter_forwards() {
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut adapter = NetVideoSender::new(FakeNetSender {
            sent: std::sync::Arc::clone(&sent),
        });
        assert!(adapter
            .send_frame(EncodedFrame {
                frame_id: 1,
                pts_us: 0,
                capture_ts_us: 0,
                is_key: true,
                slices: vec![Bytes::from_static(b"x")],
            })
            .is_ok());
        assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn input_error_code_is_stable() {
        assert_eq!(InputError::ViGEmNotInstalled.code(), 8001);
    }
}
