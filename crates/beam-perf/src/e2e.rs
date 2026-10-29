//! 端到端（E2E）延迟测量：回环上屏时间戳交叉校验（§3.13）。
//!
//! §3.13 要求「E2E 测量：NTP 式四时间戳时钟同步（每 5 s）+ 回环上屏时间戳
//! 交叉校验 + 可选光学实测」。时钟同步部分见 [`crate::clock`]，本模块负责
//! 后两项。
//!
//! ## 两条独立测量路径
//!
//! E2E 延迟应当由**两条互相独立**的路径各测一遍，再比对偏差：
//!
//! 1. **时间戳路径**（时钟偏移换算）：用同一次时钟同步得到的偏移，把
//!    「主机抓屏时刻」换算到客户端时钟，再与「客户端上屏时刻」相减：
//!    `e2e = client_present_ts - (host_capture_ts - offset)`。
//!    偏移见 [`crate::clock::ClockSyncEstimate`]，定义是「服务器时钟 - 客户端时钟」。
//! 2. **分段路径**（各阶段实测耗时求和）：把同一帧在各阶段的实测耗时
//!    （抓屏 / 颜色转换 / 编码 / 分包 / 网络单程 / 解码 / 上屏）相加。
//!    这条路径完全不依赖时钟偏移，因此可以作为时间戳路径的交叉校验基准。
//!
//! 两条路径的差值即 [`E2ECrossCheck::deviation_us`]。正常情况下它应当落在
//! 时钟偏移估计误差与分段计时误差的量级内；偏差持续偏大说明偏移估计失准
//! （典型原因：链路上下行不对称，见 [`crate::clock`] 模块文档）。
//!
//! ## 光学实测：**当前未实现**
//!
//! 光学实测是**硬件相关**的，不在本 crate 内实现，也不能凭空造数。
//! 需要的设备与流程如下（本 crate 只提供承载结构与换算逻辑，采集流程
//! 不存在）：
//!
//! - **设备**：一台能同时拍到主机屏幕与客户端屏幕的高速摄像机
//!   （≥ 500 fps，需覆盖两端显示器的同一视野），或两台做硬触发同步的
//!   高速相机；一个可编程 LED / 光电二极管同步信标。
//! - **同步信标**：主机在每帧上叠加（或串接一路闪光灯）发射**同步闪**，
//!   相机画面里可定位该闪光的帧号，得到「主机抓屏时刻」在相机时间轴上的位置。
//! - **客户端标记**：客户端画面叠加**帧序号水印**（与串流帧 id 一致），
//!   用于在相机时间轴上定位「该帧上屏时刻」。
//! - **后处理**：由相机时间轴上的两个时间点相减，得到完全**不依赖任何软件
//!   时钟**的 E2E，作为第三方基准，与本模块的两条软件路径三方比对。
//!
//! 采集到数据后，用 [`OpticalMeasurement`] 承载并交给
//! [`E2ECrossCheck::from_parts`] 的 `optical` 参数即可接入比对。
//! 在具备上述硬件之前，该路径始终为 `None`，**不会**用估算值顶替。

use std::time::Duration;

use crate::clock::ClockSyncEstimate;

/// 一帧在各阶段的实测耗时，用于「分段路径」求和。
///
/// 字段与 [`crate::Stage`] 的阶段一一对应；`None` 表示该阶段本次没有
/// 独立计时（例如未接网络单程时序），求和时跳过该字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FramePipelineDurations {
    /// 抓屏耗时。
    pub capture: Option<Duration>,
    /// 颜色转换耗时。
    pub color_convert: Option<Duration>,
    /// 编码耗时。
    pub encode: Option<Duration>,
    /// 分包 + FEC 编码耗时。
    pub packetize: Option<Duration>,
    /// 网络单程耗时（一般由 RTT / 2 得到）。
    pub network_one_way: Option<Duration>,
    /// 解码耗时。
    pub decode: Option<Duration>,
    /// 上屏耗时。
    pub render: Option<Duration>,
}

impl FramePipelineDurations {
    /// 各阶段耗时之和，微秒。
    ///
    /// 所有阶段都没有计时数据时返回 `None`（**不返回 0 冒充有效值**）。
    pub fn total_us(&self) -> Option<u64> {
        let parts: [Option<Duration>; 7] = [
            self.capture,
            self.color_convert,
            self.encode,
            self.packetize,
            self.network_one_way,
            self.decode,
            self.render,
        ];
        let mut total: u128 = 0;
        let mut any = false;
        for p in parts.into_iter().flatten() {
            any = true;
            total += p.as_micros();
        }
        if !any {
            return None;
        }
        Some(total.min(u64::MAX as u128) as u64)
    }
}

/// 光学实测结果（由外部高速相机采集流程提供）。
///
/// 本 crate **不实现**采集流程（见模块文档「光学实测：**当前未实现**」），
/// 只提供承载结构：外部流程填好三个时间戳后即可接入三方比对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpticalMeasurement {
    /// 主机抓屏时刻在**相机时间轴**上的位置，微秒。
    pub host_capture_ts_us: u64,
    /// 客户端上屏时刻在**相机时间轴**上的位置，微秒。
    pub client_present_ts_us: u64,
    /// 相机帧号（便于回溯原始视频帧）。
    pub camera_frame: u64,
}

impl OpticalMeasurement {
    /// 光学路径测得的 E2E，微秒。
    ///
    /// 两个时间戳必须来自同一条相机时间轴；否则结果没有意义。
    /// 客户端上屏时刻早于主机抓屏时刻时返回 `None`（数据不自洽）。
    pub fn e2e_us(&self) -> Option<u64> {
        self.client_present_ts_us
            .checked_sub(self.host_capture_ts_us)
    }
}

/// E2E 交叉校验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct E2ECrossCheck {
    /// 时间戳路径（时钟偏移换算）测得的 E2E，微秒。
    pub clock_based_us: u64,
    /// 分段路径（各阶段实测耗时求和）测得的 E2E，微秒。
    pub component_based_us: u64,
    /// 两条软件路径的偏差，微秒（时间戳路径 - 分段路径）。
    pub deviation_us: i64,
    /// 光学路径测得的 E2E，微秒；未接入光学实测时为 `None`。
    pub optical_us: Option<u64>,
    /// 光学路径与时间戳路径的偏差，微秒；未接入光学实测时为 `None`。
    pub optical_deviation_us: Option<i64>,
}

impl E2ECrossCheck {
    /// 汇总三路（最多三路）测量结果。
    pub fn from_parts(
        clock_based_us: u64,
        component_based_us: u64,
        optical_us: Option<u64>,
    ) -> Self {
        Self {
            clock_based_us,
            component_based_us,
            deviation_us: clock_based_us as i64 - component_based_us as i64,
            optical_us,
            optical_deviation_us: optical_us.map(|o| clock_based_us as i64 - o as i64),
        }
    }
}

/// 用时钟偏移把「主机抓屏时刻 → 客户端上屏时刻」换算成 E2E，微秒。
///
/// `host_capture_ts_us` 是主机（服务器）时钟上的抓屏时刻，
/// `client_present_ts_us` 是客户端时钟上的上屏时刻，
/// `offset_us` 是「服务器时钟 - 客户端时钟」的偏移估计
/// （见 [`crate::clock::ClockSyncEstimate::offset_us`]）。
///
/// 换算结果为负（说明两端时钟或时间戳不自洽）时返回 `None`。
pub fn clock_based_e2e_us(
    host_capture_ts_us: u64,
    client_present_ts_us: u64,
    offset_us: i64,
) -> Option<u64> {
    // 主机抓屏时刻换算到客户端时钟：client = server - offset。
    let capture_in_client = host_capture_ts_us as i64 - offset_us;
    let e2e = client_present_ts_us as i64 - capture_in_client;
    if e2e < 0 {
        return None;
    }
    Some(e2e as u64)
}

/// 用时钟同步的估计值做换算（便捷重载）。
pub fn clock_based_e2e_from_estimate(
    host_capture_ts_us: u64,
    client_present_ts_us: u64,
    estimate: &ClockSyncEstimate,
) -> Option<u64> {
    clock_based_e2e_us(host_capture_ts_us, client_present_ts_us, estimate.offset_us)
}

/// 三路测量的统一入口：时间戳路径 + 分段路径 +（可选）光学路径。
///
/// 返回 `None` 表示缺少必要数据（时间戳路径或分段路径任一无法算出）。
pub fn cross_check(
    host_capture_ts_us: u64,
    client_present_ts_us: u64,
    estimate: &ClockSyncEstimate,
    durations: &FramePipelineDurations,
    optical: Option<&OpticalMeasurement>,
) -> Option<E2ECrossCheck> {
    let clock_based =
        clock_based_e2e_from_estimate(host_capture_ts_us, client_present_ts_us, estimate)?;
    let component_based = durations.total_us()?;
    let optical_us = optical.and_then(|o| o.e2e_us());
    Some(E2ECrossCheck::from_parts(
        clock_based,
        component_based,
        optical_us,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate(offset_us: i64) -> ClockSyncEstimate {
        ClockSyncEstimate {
            rtt_us: 20_000,
            offset_us,
            one_way_delay_us: 10_000,
        }
    }

    #[test]
    fn clock_based_e2e_uses_offset() {
        // 主机时钟比客户端快 40 ms：主机抓屏时刻换算到客户端要减 40 ms。
        // 主机抓屏 1.000 s（主机时钟），客户端上屏 1.020 s（客户端时钟）。
        // 客户端时钟上的抓屏时刻 = 1.000 - 0.040 = 0.960 s，E2E = 60 ms。
        let e2e = clock_based_e2e_us(1_000_000, 1_020_000, 40_000);
        assert_eq!(e2e, Some(60_000));
    }

    #[test]
    fn clock_based_e2e_without_offset_is_plain_difference() {
        assert_eq!(clock_based_e2e_us(1_000_000, 1_050_000, 0), Some(50_000));
    }

    #[test]
    fn clock_based_e2e_rejects_inconsistent_timestamps() {
        // 换算后抓屏时刻晚于上屏时刻 → 无效。
        assert_eq!(clock_based_e2e_us(1_000_000, 900_000, 0), None);
    }

    #[test]
    fn component_sum_adds_present_stages_only() {
        let d = FramePipelineDurations {
            capture: Some(Duration::from_millis(2)),
            encode: Some(Duration::from_millis(3)),
            network_one_way: Some(Duration::from_millis(10)),
            ..Default::default()
        };
        assert_eq!(d.total_us(), Some(15_000));
    }

    #[test]
    fn component_sum_is_none_when_nothing_measured() {
        assert_eq!(FramePipelineDurations::default().total_us(), None);
    }

    #[test]
    fn cross_check_reports_deviation() {
        let est = estimate(0);
        let durations = FramePipelineDurations {
            capture: Some(Duration::from_millis(5)),
            render: Some(Duration::from_millis(5)),
            ..Default::default()
        };
        // 主机抓屏 1.000 s，客户端上屏 1.030 s → 时间戳路径 30 ms。
        // 分段路径合计 10 ms → 偏差 20 ms。
        let r = cross_check(1_000_000, 1_030_000, &est, &durations, None).expect("应当能交叉校验");
        assert_eq!(r.clock_based_us, 30_000);
        assert_eq!(r.component_based_us, 10_000);
        assert_eq!(r.deviation_us, 20_000);
        assert_eq!(r.optical_us, None);
        assert_eq!(r.optical_deviation_us, None);
    }

    #[test]
    fn cross_check_includes_optical_when_available() {
        let est = estimate(0);
        let durations = FramePipelineDurations {
            render: Some(Duration::from_millis(30)),
            ..Default::default()
        };
        let optical = OpticalMeasurement {
            host_capture_ts_us: 5_000_000,
            client_present_ts_us: 5_032_000,
            camera_frame: 42,
        };
        let r = cross_check(1_000_000, 1_030_000, &est, &durations, Some(&optical))
            .expect("应当能交叉校验");
        assert_eq!(r.optical_us, Some(32_000));
        assert_eq!(r.optical_deviation_us, Some(-2_000));
    }

    #[test]
    fn optical_measurement_rejects_inverted_timestamps() {
        let optical = OpticalMeasurement {
            host_capture_ts_us: 5_032_000,
            client_present_ts_us: 5_000_000,
            camera_frame: 42,
        };
        assert_eq!(optical.e2e_us(), None);
    }

    #[test]
    fn cross_check_needs_both_software_paths() {
        let est = estimate(0);
        // 分段路径没有任何数据 → 无法交叉校验。
        let r = cross_check(
            1_000_000,
            1_030_000,
            &est,
            &FramePipelineDurations::default(),
            None,
        );
        assert_eq!(r, None);
    }
}
