//! 平台实现 → 会话 trait 的适配器（§3.9「通过 trait 注入，便于测试」）。

use beam_decode::VideoDecoder;
use beam_proto::{ControlMsg, EncodedFrame, InputEvent};
use beam_render::Renderer;
use beam_session::{FrameConsumer, SessionError, Transport, TransportFactory};

/// 解码 → 上屏适配器。
#[derive(Debug)]
pub struct DecodeRenderConsumer<D, R> {
    decoder: D,
    renderer: R,
}

impl<D, R> DecodeRenderConsumer<D, R>
where
    D: VideoDecoder,
    R: Renderer,
{
    /// 组装适配器。
    pub fn new(decoder: D, renderer: R) -> Self {
        Self { decoder, renderer }
    }

    /// 访问底层解码器。
    pub fn decoder(&self) -> &D {
        &self.decoder
    }

    /// 访问底层渲染器。
    pub fn renderer(&self) -> &R {
        &self.renderer
    }
}

impl<D, R> FrameConsumer for DecodeRenderConsumer<D, R>
where
    D: VideoDecoder + Send,
    R: Renderer + Send,
{
    fn consume(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        let assembled = beam_proto::AssembledFrame {
            frame_id: f.frame_id,
            pts_us: f.pts_us,
            capture_ts_us: f.capture_ts_us,
            is_key: f.is_key,
            payload: f.to_payload(),
        };
        let decoded =
            self.decoder
                .decode(assembled)
                .map_err(|e| SessionError::TransportFailed {
                    reason: format!("解码失败 {}: {e}", e.code_str()),
                })?;
        self.renderer
            .present(&decoded)
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("上屏失败 {}: {e}", e.code_str()),
            })
    }
}

/// 基于 QUIC 连接的会话传输适配器。
#[derive(Debug)]
pub struct NetTransport {
    conn: beam_net::Conn,
}

impl NetTransport {
    /// 包装一条 QUIC 连接。
    pub fn new(conn: beam_net::Conn) -> Self {
        Self { conn }
    }

    /// 访问底层连接。
    pub fn conn(&self) -> &beam_net::Conn {
        &self.conn
    }
}

impl Transport for NetTransport {
    fn send_control(&mut self, _msg: ControlMsg) -> Result<(), SessionError> {
        // 控制消息走 QUIC 可靠流；本轮只保留契约。
        Ok(())
    }

    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        use beam_net::VideoSender;
        self.conn
            .send_frame(f)
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("发送画面失败 {}: {e}", e.code_str()),
            })
    }

    fn send_input(&mut self, _ev: InputEvent) -> Result<(), SessionError> {
        // 输入走独立的单向可靠流；本轮只保留契约。
        Ok(())
    }

    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        None
    }

    fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
        None
    }
}

/// 传输工厂适配器：建立一次 QUIC 连接并包装成 [`NetTransport`]。
#[derive(Debug)]
pub struct NetTransportFactory {
    addr: std::net::SocketAddr,
    cfg: beam_net::NetConfig,
    verifier: std::sync::Arc<dyn beam_net::PeerVerifier>,
}

impl NetTransportFactory {
    /// 组装工厂。
    pub fn new(
        addr: std::net::SocketAddr,
        cfg: beam_net::NetConfig,
        verifier: std::sync::Arc<dyn beam_net::PeerVerifier>,
    ) -> Self {
        Self {
            addr,
            cfg,
            verifier,
        }
    }
}

// beam-client 不直接依赖 beam-pair 时用到的类型由调用方注入；
// 这里通过泛型避免额外的 crate 依赖。
impl TransportFactory for NetTransportFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
        let conn = futures::executor::block_on(beam_net::NetEndpoint::connect(
            self.addr,
            self.cfg.clone(),
            self.verifier.clone(),
        ))
        .map_err(|e| SessionError::TransportFailed {
            reason: format!("连接失败 {}: {e}", e.code_str()),
        })?;
        Ok(Box::new(NetTransport::new(conn)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_decode::{D3D11VaDecoder, DecError};
    use beam_render::{D3D11Renderer, RenderError};
    use bytes::Bytes;

    struct FakeDecoder;
    impl VideoDecoder for FakeDecoder {
        fn decode(
            &mut self,
            f: beam_proto::AssembledFrame,
        ) -> Result<beam_proto::DecodedFrame, DecError> {
            Ok(beam_proto::DecodedFrame {
                frame_id: f.frame_id,
                pts_us: f.pts_us,
                capture_ts_us: f.capture_ts_us,
                width: 1920,
                height: 1080,
                is_key: f.is_key,
                surface: beam_proto::PlatformHandle::NONE,
            })
        }
        fn reset(&mut self) -> Result<(), DecError> {
            Ok(())
        }
    }

    struct FakeRenderer {
        presented: usize,
    }
    impl Renderer for FakeRenderer {
        fn present(&mut self, _f: &beam_proto::DecodedFrame) -> Result<(), RenderError> {
            self.presented += 1;
            Ok(())
        }
    }

    #[test]
    fn consumer_decodes_and_presents() {
        let mut consumer = DecodeRenderConsumer::new(FakeDecoder, FakeRenderer { presented: 0 });
        let frame = EncodedFrame {
            frame_id: 1,
            pts_us: 10,
            capture_ts_us: 5,
            is_key: true,
            slices: vec![Bytes::from_static(b"nal")],
        };
        consumer.consume(frame).expect("应当能解码并上屏");
        assert_eq!(consumer.renderer().presented, 1);
    }

    #[test]
    fn consumer_propagates_decode_error() {
        let mut consumer = DecodeRenderConsumer::new(D3D11VaDecoder::new(), D3D11Renderer::new());
        let frame = EncodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            is_key: true,
            slices: vec![Bytes::from_static(b"nal")],
        };
        let err = consumer.consume(frame).expect_err("未启用后端必须失败");
        assert_eq!(err.code(), 9005);
    }
}
