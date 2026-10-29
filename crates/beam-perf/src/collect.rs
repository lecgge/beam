//! 采集实现（仅 `perf-stats` 开启时编译）。
//!
//! §3.13：采集开销 < 1% CPU，单点 < 200 ns。
//! 因此这里只用互斥锁保护的定长桶，不做排序、不做格式化。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::histogram::Histogram;
use crate::ring::{RingBuffer, Sample};
use crate::stage::Stage;
use crate::sysinfo::{SystemUsage, SystemUsageSource};
use crate::{ExportFormat, Perf, PerfError, PerfSnapshot, StageSnapshot};

/// 全局采集器。
#[derive(Debug)]
pub struct Collector {
    histograms: HashMap<Stage, Histogram>,
    ring: RingBuffer,
    bitrate_bps: u32,
    fps: u32,
    dropped_frames: u32,
    /// 最近一次系统资源占用采样；未采样时为 `None`。
    system: Option<SystemUsage>,
    start: Instant,
}

impl Default for Collector {
    fn default() -> Self {
        Self {
            histograms: HashMap::new(),
            ring: RingBuffer::default(),
            bitrate_bps: 0,
            fps: 0,
            dropped_frames: 0,
            system: None,
            start: Instant::now(),
        }
    }
}

fn collector() -> &'static Mutex<Collector> {
    static COLLECTOR: OnceLock<Mutex<Collector>> = OnceLock::new();
    COLLECTOR.get_or_init(|| Mutex::new(Collector::default()))
}

/// 已注册的系统资源占用采样源。
///
/// 未注册时采样入口返回 `None`（**不返回假数据**）。
fn system_usage_source() -> &'static RwLock<Option<Arc<dyn SystemUsageSource>>> {
    static SOURCE: OnceLock<RwLock<Option<Arc<dyn SystemUsageSource>>>> = OnceLock::new();
    SOURCE.get_or_init(|| RwLock::new(None))
}

/// 注册系统资源占用采样源（后注册覆盖先注册）。
pub fn set_system_usage_source(source: Arc<dyn SystemUsageSource>) {
    let Ok(mut guard) = system_usage_source().write() else {
        return;
    };
    *guard = Some(source);
}

/// 采样一次系统资源占用。
///
/// 没有注册采样源，或采样源报告不可用时返回 `None`。
pub fn sample_system_usage() -> Option<SystemUsage> {
    let source = {
        let guard = system_usage_source().read().ok()?;
        guard.clone()
    }?;
    let usage = source.sample()?;
    let Ok(mut c) = collector().lock() else {
        return Some(usage);
    };
    c.system = Some(usage.clone());
    Some(usage)
}

/// 记录一次阶段耗时。
pub fn record(stage: Stage, dur: Duration) {
    let Ok(mut c) = collector().lock() else {
        return;
    };
    c.histograms.entry(stage).or_default().record(dur);
    let ts_us = c.start.elapsed().as_micros() as u64;
    c.ring.push(Sample {
        ts_us,
        stage,
        value_ns: dur.as_nanos().min(u64::MAX as u128) as u64,
    });
}

/// 记录码率 / 帧率 / 掉帧。
pub fn set_counters(bitrate_bps: u32, fps: u32, dropped_frames: u32) {
    let Ok(mut c) = collector().lock() else {
        return;
    };
    c.bitrate_bps = bitrate_bps;
    c.fps = fps;
    c.dropped_frames = dropped_frames;
}

/// 清空统计。
pub fn reset() {
    let Ok(mut c) = collector().lock() else {
        return;
    };
    c.histograms.clear();
    c.ring.clear();
    c.bitrate_bps = 0;
    c.fps = 0;
    c.dropped_frames = 0;
    c.system = None;
    c.start = Instant::now();
}

/// 取当前快照。
pub fn snapshot() -> PerfSnapshot {
    let Ok(c) = collector().lock() else {
        return PerfSnapshot::default();
    };
    let mut stages: Vec<(Stage, StageSnapshot)> = c
        .histograms
        .iter()
        .map(|(stage, h)| {
            (
                *stage,
                StageSnapshot {
                    count: h.count,
                    mean_ns: h.mean().as_nanos() as u64,
                    p50_ns: h.p50().as_nanos() as u64,
                    p95_ns: h.p95().as_nanos() as u64,
                    p99_ns: h.p99().as_nanos() as u64,
                    max_ns: h.max_ns,
                },
            )
        })
        .collect();
    stages.sort_by_key(|(s, _)| *s);
    PerfSnapshot {
        stages,
        samples: c.ring.len(),
        bitrate_bps: c.bitrate_bps,
        fps: c.fps,
        dropped_frames: c.dropped_frames,
        system: c.system.clone(),
    }
}

/// 导出到文件。
pub fn export(fmt: ExportFormat, path: &Path) -> Result<(), PerfError> {
    let snapshot = snapshot();
    let content = match fmt {
        ExportFormat::Csv => to_csv(&snapshot),
        ExportFormat::Json => {
            serde_json::to_string_pretty(&snapshot_json(&snapshot)).map_err(|e| {
                PerfError::ExportFailed {
                    reason: format!("序列化 JSON 失败: {e}"),
                }
            })?
        }
    };
    std::fs::write(path, content).map_err(|e| PerfError::ExportFailed {
        reason: format!("写入 {} 失败: {e}", path.display()),
    })
}

fn to_csv(snapshot: &PerfSnapshot) -> String {
    let mut out = String::from("stage,count,mean_ns,p50_ns,p95_ns,p99_ns,max_ns\n");
    for (stage, s) in &snapshot.stages {
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            stage.as_str(),
            s.count,
            s.mean_ns,
            s.p50_ns,
            s.p95_ns,
            s.p99_ns,
            s.max_ns
        ));
    }
    out.push_str(&format!(
        "# bitrate_bps={},fps={},dropped_frames={},samples={}\n",
        snapshot.bitrate_bps, snapshot.fps, snapshot.dropped_frames, snapshot.samples
    ));
    out.push_str(&system_csv(snapshot.system.as_ref()));
    out
}

/// 系统资源占用的 CSV 片段（注释行）。
///
/// 采样不可用时写出 `unavailable`，**不写 0**（0 会被误读成「确实没有占用」）。
fn system_csv(system: Option<&SystemUsage>) -> String {
    match system {
        None => String::from("# system=cpu:unavailable,gpu:unavailable,memory:unavailable\n"),
        Some(u) => {
            let cpu = u
                .cpu_percent
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "unavailable".to_string());
            let gpu = u
                .gpu_percent
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "unavailable".to_string());
            let mem = match (u.memory_used_bytes, u.memory_total_bytes) {
                (Some(used), Some(total)) => format!("{used}/{total}"),
                (Some(used), None) => format!("{used}/unavailable"),
                (None, Some(total)) => format!("unavailable/{total}"),
                (None, None) => "unavailable".to_string(),
            };
            format!("# system=cpu:{cpu},gpu:{gpu},memory:{mem}\n")
        }
    }
}

fn snapshot_json(snapshot: &PerfSnapshot) -> serde_json::Value {
    serde_json::json!({
        "bitrate_bps": snapshot.bitrate_bps,
        "fps": snapshot.fps,
        "dropped_frames": snapshot.dropped_frames,
        "samples": snapshot.samples,
        "system": snapshot.system.as_ref().map(|u| serde_json::json!({
            "cpu_percent": u.cpu_percent,
            "gpu_percent": u.gpu_percent,
            "memory_used_bytes": u.memory_used_bytes,
            "memory_total_bytes": u.memory_total_bytes,
        })),
        "stages": snapshot.stages.iter().map(|(stage, s)| serde_json::json!({
            "stage": stage.as_str(),
            "count": s.count,
            "mean_ns": s.mean_ns,
            "p50_ns": s.p50_ns,
            "p95_ns": s.p95_ns,
            "p99_ns": s.p99_ns,
            "max_ns": s.max_ns,
        })).collect::<Vec<_>>(),
    })
}

/// 让 `Perf` 的公共 API 指向本模块。
#[allow(dead_code)]
fn _assert_perf_links() {
    let _ = Perf::snapshot();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 采集器是全局单例，测试之间必须串行。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn records_are_reflected_in_snapshot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        record(Stage::Encode, Duration::from_millis(2));
        record(Stage::Encode, Duration::from_millis(4));
        let s = snapshot();
        let (_, enc) = s
            .stages
            .iter()
            .find(|(st, _)| *st == Stage::Encode)
            .expect("应当有编码阶段");
        assert_eq!(enc.count, 2);
        assert_eq!(enc.max_ns, 4_000_000);
        reset();
    }

    #[test]
    fn export_writes_csv_and_json() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        record(Stage::Decode, Duration::from_millis(1));
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let csv = dir.path().join("perf.csv");
        export(ExportFormat::Csv, &csv).expect("应当能导出 CSV");
        let text = std::fs::read_to_string(&csv).expect("应当能读");
        assert!(text.contains("decode"));

        let json = dir.path().join("perf.json");
        export(ExportFormat::Json, &json).expect("应当能导出 JSON");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json).expect("应当能读"))
                .expect("应当是 JSON");
        assert!(!value["stages"]
            .as_array()
            .expect("应当有阶段数组")
            .is_empty());
        reset();
    }

    /// 固定返回一份数据的采样源。
    struct FixedSource(Option<SystemUsage>);

    impl SystemUsageSource for FixedSource {
        fn sample(&self) -> Option<SystemUsage> {
            self.0.clone()
        }
    }

    #[test]
    fn sample_without_source_returns_none() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 未注册采样源 → 采样不可用，返回 None 而不是假数据。
        set_system_usage_source(Arc::new(FixedSource(None)));
        assert_eq!(sample_system_usage(), None);
        assert_eq!(snapshot().system, None);
    }

    #[test]
    fn sampled_usage_reaches_snapshot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let usage = SystemUsage {
            cpu_percent: Some(12.5),
            gpu_percent: Some(30.0),
            memory_used_bytes: Some(1024),
            memory_total_bytes: Some(2048),
        };
        set_system_usage_source(Arc::new(FixedSource(Some(usage.clone()))));
        let got = sample_system_usage().expect("应当能采样");
        assert_eq!(got, usage);
        let s = snapshot();
        assert_eq!(s.system, Some(usage));
        reset();
    }

    #[test]
    fn unavailable_source_does_not_fake_numbers() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        set_system_usage_source(Arc::new(FixedSource(None)));
        assert_eq!(sample_system_usage(), None);
        let s = snapshot();
        assert_eq!(s.system, None);
        // 快照里不应出现 0 之类的占位数字。
        assert!(s.system.is_none());
        reset();
    }

    #[test]
    fn csv_marks_unavailable_system_metrics() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let csv = dir.path().join("perf.csv");
        export(ExportFormat::Csv, &csv).expect("应当能导出 CSV");
        let text = std::fs::read_to_string(&csv).expect("应当能读");
        assert!(
            text.contains("cpu:unavailable"),
            "采样不可用时应写 unavailable，实际：{text}"
        );
        reset();
    }
}
