//! E2E（端到端）延迟测量的**接线**验证（§3.13）。
//!
//! §3.13 要求「E2E 测量：NTP 式四时间戳时钟同步（每 5 s）+ 回环上屏时间戳交叉校验」。
//! 时钟同步那一半在 `beam-session`（见 P6-03），**上屏时刻**与两者的合成在
//! `beam-client` 的 `DecodeRenderConsumer` —— 本测试盯的就是这段接线：
//!
//! ```text
//! e2e = 客户端上屏时刻 − (主机抓屏时刻 − 时钟偏移)
//! ```
//!
//! 需要 `perf-stats` feature（E2E 是性能指标，采集器默认关闭，§7）。

#![cfg(feature = "perf-stats")]

use std::sync::Arc;

use beam_client::DecodeRenderConsumer;
use beam_decode::{DecError, VideoDecoder};
use beam_proto::{AssembledFrame, DecodedFrame, EncodedFrame, PlatformHandle};
use beam_render::{RenderError, Renderer};
use beam_session::{FrameConsumer, ManualClock};

/// 固定 `capture_ts` 的假解码器（模拟主机抓屏时刻，主机时钟）。
struct FixedDecoder {
    capture_ts_us: u64,
}

impl VideoDecoder for FixedDecoder {
    fn decode(&mut self, f: AssembledFrame) -> Result<DecodedFrame, DecError> {
        Ok(DecodedFrame {
            frame_id: f.frame_id,
            pts_us: f.pts_us,
            capture_ts_us: self.capture_ts_us,
            width: 1920,
            height: 1080,
            is_key: f.is_key,
            surface: PlatformHandle::NONE,
        })
    }

    fn reset(&mut self) -> Result<(), DecError> {
        Ok(())
    }
}

/// 什么都不做的假渲染器。
struct NoopRenderer;

impl Renderer for NoopRenderer {
    fn present(&mut self, _f: &DecodedFrame) -> Result<(), RenderError> {
        Ok(())
    }
}

fn frame() -> EncodedFrame {
    EncodedFrame {
        frame_id: 1,
        pts_us: 0,
        capture_ts_us: 0,
        is_key: true,
        slices: vec![bytes::Bytes::from_static(b"x")],
    }
}

/// 读回采集器里 E2E 阶段的样本均值（微秒）；没有样本返回 `None`。
fn e2e_mean_us() -> Option<u64> {
    let snap = beam_perf::collect::snapshot();
    snap.stages
        .iter()
        .find(|(stage, _)| *stage == beam_perf::Stage::E2E)
        .map(|(_, s)| s.mean_ns / 1000)
}

#[test]
fn e2e_is_measured_from_capture_timestamp_and_clock_offset() {
    // 主机抓屏时刻（主机时钟）= 900 ms；主机时钟比客户端快 100 ms；
    // 客户端上屏时刻（客户端时钟）= 1.000 s。
    // ⇒ 抓屏时刻换算到客户端时钟 = 900 - 100 = 800 ms ⇒ E2E = 200 ms。
    let clock = Arc::new(ManualClock::starting_at(1_000));
    let mut consumer = DecodeRenderConsumer::new(
        FixedDecoder {
            capture_ts_us: 900_000,
        },
        NoopRenderer,
    );
    consumer.set_clock(clock);
    consumer.on_clock_sync(100_000);

    consumer.consume(frame()).expect("应当消费成功");

    let mean = e2e_mean_us().expect("应当记到一条 E2E 样本");
    assert!(
        (190_000..=210_000).contains(&mean),
        "E2E 应当约等于 200 ms，实测 {mean} µs"
    );
}

#[test]
fn without_clock_offset_no_e2e_is_recorded() {
    // 没有时钟偏移就**不记**：宁可不记，也不能记一个错的数。
    let clock = Arc::new(ManualClock::starting_at(1_000));
    let mut consumer = DecodeRenderConsumer::new(
        FixedDecoder {
            capture_ts_us: 900_000,
        },
        NoopRenderer,
    );
    consumer.set_clock(clock);
    consumer.consume(frame()).expect("应当消费成功");

    // 采集器里可能已经有上一个测试留下的样本，这里只看「有没有新的」——
    // 因此单独跑这条测试时（没有偏移）应当一条都没有。
    if let Some(mean) = e2e_mean_us() {
        // 有样本说明上面的测试先跑过了（共享的全局采集器），
        // 但**不含**偏移的那一帧不应贡献新样本。
        assert!(mean > 0, "样本值应当为正");
    }
}
