//! 性能埋点接入（§3.13）。
//!
//! §3.13 要求把「抓屏 / 颜色转换 / 编码 / 分包 / RTT / 丢包 / FEC 恢复 /
//! 抖动缓冲 / 解码 / 渲染 / 端到端」各阶段的耗时采集起来。
//! **本 crate 负责其中的「渲染」阶段**；其余阶段由对应层负责
//! （抓屏在 beam-capture，颜色转换 / 编码在 beam-encode，解码在 beam-decode，
//! 分包 / RTT / 丢包 / FEC 恢复 / 抖动缓冲在 beam-net）。
//!
//! ## 门控（§3.13 / §7）
//!
//! - `perf-stats` feature **默认关闭**；关闭时 `beam-perf` 根本不在依赖树里
//!   （可选依赖），本模块的 [`perf_span!`] 展开为空（`()`）——
//!   **零开销**：不读时钟、不分配、不加锁；
//! - 开启时 [`perf_span!`] 展开为 `beam-perf` 的作用域计时守卫，
//!   离开作用域时把耗时记进全局采集器，最终出现在 `Perf::snapshot()` 里。
//!
//! ## 用法
//!
//! ```ignore
//! let _render_span = perf_span!("render");
//! // ……被测量的工作……
//! // 作用域结束时自动记录耗时。
//! ```

/// 作用域计时宏：`perf_span!("render")`。
///
/// - `perf-stats` **开启**：展开为 `beam_perf::perf_span!` 的计时守卫；
/// - **关闭**：展开为空（`()`），**零开销**。
///
/// 用法：`let _span = perf_span!("render");`
#[allow(
    unused_macros,
    reason = "真实渲染后端（d3d11 feature）关闭时没有调用点，此时宏确实未被使用"
)]
macro_rules! perf_span {
    ($stage:literal) => {{
        #[cfg(feature = "perf-stats")]
        let beam_perf_guard = beam_perf::perf_span!($stage);
        #[cfg(not(feature = "perf-stats"))]
        let beam_perf_guard = ();
        beam_perf_guard
    }};
}

#[cfg(test)]
mod tests {
    /// §3.13：`perf_span!` 在 `perf-stats` 关闭时必须展开为空（`()`）。
    #[cfg(not(feature = "perf-stats"))]
    #[test]
    fn span_expands_to_unit_when_feature_disabled() {
        let _: () = perf_span!("render");
    }

    /// `perf-stats` 开启时，`perf_span!` 展开为 `beam-perf` 的作用域计时守卫。
    #[cfg(feature = "perf-stats")]
    #[test]
    fn span_expands_to_guard_when_feature_enabled() {
        let _: beam_perf::Span = perf_span!("render");
    }

    /// 埋点必须真的把数据汇进 `Perf::snapshot()`（§3.13）。
    #[cfg(feature = "perf-stats")]
    #[test]
    fn recorded_span_reaches_global_snapshot() {
        beam_perf::Perf::reset();
        {
            let _span = perf_span!("render");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let snapshot = beam_perf::Perf::snapshot();
        let (_, render) = snapshot
            .stages
            .iter()
            .find(|(stage, _)| *stage == beam_perf::Stage::Render)
            .expect("渲染阶段应当出现在快照里");
        assert!(render.count >= 1, "应当至少记录一次");
        assert!(
            render.max_ns >= 2_000_000,
            "采样值应当反映真实等待时长，实际 {} ns",
            render.max_ns
        );
        beam_perf::Perf::reset();
    }
}
