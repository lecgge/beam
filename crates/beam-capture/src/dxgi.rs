//! DXGI 桌面复制（Desktop Duplication）后端（§3.5 回退路径）。
//!
//! ## 什么时候用
//!
//! §3.5 的错误码表规定 `E5002`「独占全屏无法抓取 → 切 DXGI」。
//! 当目标应用处于独占全屏模式时 WGC 拿不到画面，但 DXGI 的桌面复制
//! 仍然可以拿到（代价是拿不到光标与独占全屏图层）。
//!
//! ## 线程模型
//!
//! DXGI 桌面复制没有回调线程，是**拉取式**的：
//! 编码节拍线程调用 [`DxgiBackend::latest`] 时用 `AcquireNextFrame(0)`
//! （零超时，非阻塞）尝试取一帧，取不到就返回 `None`。
//! 取到的帧同样是「GPU→GPU 拷贝」到一块每帧新建的目标纹理，
//! 保证交给消费者的纹理不会被下一帧覆盖。
//!
//! ## 限制
//!
//! - 桌面复制只能复制**整个输出（output）**的画面，再按显示器矩形裁剪；
//! - 独占全屏的图层不会被复制进来；
//! - 光标不会被复制进来（§3.5 的「光标单独通道」在这里天然成立）。
//!
//! ## 性能预算
//!
//! §3.5 的预算（抓屏+取纹理 T1 ≤1.5 ms、T2/T3 ≤1 ms）**没有在本机实测**。

// 本模块需要 unsafe：DXGI / D3D11 都是裸 COM API——枚举适配器与输出、
// 创建桌面复制对象、取帧、GPU 拷贝都是 unsafe 调用。
#![allow(unsafe_code)]

use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_9_1, D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_3,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, IDXGIOutput,
    IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_WAIT_TIMEOUT,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};

use windows_capture::monitor::Monitor;

use crate::error::CapError;
use crate::source::{now_us, CaptureConfig, CapturedFrame, Texture};

/// DXGI 桌面复制后端句柄。
pub(crate) struct DxgiBackend {
    /// 与输出同一块适配器上的 D3D11 设备。
    device: ID3D11Device,
    /// 设备上下文（GPU 拷贝在它上面排队）。
    device_context: ID3D11DeviceContext,
    /// 桌面复制对象。
    duplication: IDXGIOutputDuplication,
    /// 源（桌面图像）纹理里的目标区域。
    region: D3D11_BOX,
    /// 目标区域宽高。
    width: u32,
    height: u32,
}

impl DxgiBackend {
    /// 按配置启动 DXGI 桌面复制。
    pub(crate) fn start(cfg: &CaptureConfig) -> Result<Self, CapError> {
        let monitor = crate::source::select_monitor(cfg)?;
        let target_name = monitor.device_name().map_err(|err| {
            tracing::error!(error = %err, "读取目标显示器设备名失败");
            CapError::NoDisplay
        })?;

        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|err| {
            tracing::error!(error = %err, "创建 DXGI 工厂失败");
            CapError::DeviceLost
        })?;

        let mut adapter_index = 0u32;
        loop {
            let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(adapter) => adapter,
                Err(_) => break,
            };
            let mut output_index = 0u32;
            loop {
                let output: IDXGIOutput = match unsafe { adapter.EnumOutputs(output_index) } {
                    Ok(output) => output,
                    Err(_) => break,
                };
                let desc = unsafe { output.GetDesc() }.map_err(|err| {
                    tracing::error!(error = %err, "读取 DXGI 输出描述失败");
                    CapError::DeviceLost
                })?;
                if device_name_matches(&desc.DeviceName, &target_name) {
                    return Self::create(adapter, output, desc, &monitor);
                }
                output_index += 1;
            }
            adapter_index += 1;
        }

        tracing::error!(target = %target_name, "没有找到与目标显示器匹配的 DXGI 输出");
        Err(CapError::NoDisplay)
    }

    /// 在目标输出上建复制对象。
    fn create(
        adapter: IDXGIAdapter1,
        output: IDXGIOutput,
        output_desc: windows::Win32::Graphics::Dxgi::DXGI_OUTPUT_DESC,
        monitor: &Monitor,
    ) -> Result<Self, CapError> {
        let (device, device_context) = create_device(&adapter)?;
        let dxgi_device = device.cast::<IDXGIDevice>().map_err(|err| {
            tracing::error!(error = %err, "获取 D3D11 设备的 DXGI 接口失败");
            CapError::DeviceLost
        })?;
        // 桌面复制接口在 `IDXGIOutput1` 上（DXGI 1.2 引入）。
        let output1 = output.cast::<IDXGIOutput1>().map_err(|err| {
            tracing::error!(error = %err, "获取 DXGI 输出 1 接口失败");
            CapError::NoDisplay
        })?;
        let duplication: IDXGIOutputDuplication = unsafe { output1.DuplicateOutput(&dxgi_device) }
            .map_err(|err| {
                tracing::error!(error = %err, "创建 DXGI 桌面复制对象失败");
                CapError::DeviceLost
            })?;

        let monitor_rect = monitor_rect(monitor)?;
        // 桌面图像的原点对应输出在虚拟桌面上的原点，
        // 因此目标显示器在图像里的偏移 = 显示器矩形 - 输出矩形。
        let left = monitor_rect.left - output_desc.DesktopCoordinates.left;
        let top = monitor_rect.top - output_desc.DesktopCoordinates.top;
        let width = monitor_rect.right - monitor_rect.left;
        let height = monitor_rect.bottom - monitor_rect.top;
        if width <= 0 || height <= 0 || left < 0 || top < 0 {
            tracing::error!(
                left,
                top,
                width,
                height,
                "DXGI 输出覆盖范围与显示器矩形不匹配"
            );
            return Err(CapError::NoDisplay);
        }

        tracing::info!(width, height, "DXGI 桌面复制已启动");
        Ok(Self {
            device,
            device_context,
            duplication,
            region: D3D11_BOX {
                left: left as u32,
                top: top as u32,
                right: (left + width) as u32,
                bottom: (top + height) as u32,
                front: 0,
                back: 1,
            },
            width: width as u32,
            height: height as u32,
        })
    }

    /// 取最新一帧（非阻塞）。
    pub(crate) fn latest(&mut self) -> Option<CapturedFrame> {
        let mut frame_info = windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: 两个输出参数都是合法的栈上/堆上变量。
        let acquired = unsafe {
            self.duplication
                .AcquireNextFrame(0, &mut frame_info, &mut resource)
        };
        if let Err(err) = acquired {
            let hr = err.code();
            if hr == DXGI_ERROR_WAIT_TIMEOUT {
                // 这一拍没有新帧（正常情况）。
                return None;
            }
            if hr == DXGI_ERROR_ACCESS_LOST {
                tracing::error!("DXGI 桌面复制访问丢失(设备丢失)");
                return None;
            }
            tracing::warn!(code = hr.0, "DXGI 桌面复制取帧失败");
            return None;
        }

        let resource = match resource {
            Some(resource) => resource,
            None => {
                tracing::error!("DXGI 桌面复制返回了空资源");
                return None;
            }
        };
        let source: ID3D11Texture2D = match resource.cast() {
            Ok(texture) => texture,
            Err(err) => {
                tracing::error!(error = %err, "DXGI 桌面图像不是 D3D11 纹理");
                // 必须释放已获取的帧，否则下一次取帧会一直失败。
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return None;
            }
        };

        let destination = match self.create_destination(&source) {
            Ok(texture) => texture,
            Err(err) => {
                tracing::error!(error = %err, "创建 DXGI 目标纹理失败");
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return None;
            }
        };
        // SAFETY: 目标纹理与源纹理同设备同格式，`region` 落在源纹理范围内，
        // 满足 D3D11 `CopySubresourceRegion` 的前置条件。
        unsafe {
            self.device_context.CopySubresourceRegion(
                &destination,
                0,
                0,
                0,
                0,
                &source,
                0,
                Some(&self.region),
            )
        };
        // 复制完成后必须释放帧，否则 DXGI 最多允许一帧在途。
        if let Err(err) = unsafe { self.duplication.ReleaseFrame() } {
            tracing::warn!(error = %err, "释放 DXGI 桌面复制帧失败");
            return None;
        }

        // 消费者独占这份引用：纹理在消费者释放它之前不会被回收或改写。
        let texture: Texture = Clone::clone(&destination);
        Some(CapturedFrame {
            tex: texture,
            ts_us: now_us(),
            // 桌面复制只在桌面更新时产生新帧，因此投递出来的每帧都是「脏」的。
            dirty: true,
        })
    }

    /// 新建一块与源纹理同格式的目标纹理。
    fn create_destination(
        &self,
        source: &ID3D11Texture2D,
    ) -> Result<ID3D11Texture2D, windows::core::Error> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` 是合法的输出参数。
        unsafe { source.GetDesc(&mut desc) };
        let target_desc = D3D11_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: desc.SampleDesc,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut destination = None;
        // SAFETY: 描述符有效；输出参数在调用返回时被写入。
        unsafe {
            self.device
                .CreateTexture2D(&target_desc, None, Some(&mut destination))?
        };
        Ok(destination.expect("CreateTexture2D 成功后必须返回纹理"))
    }

    /// 停止抓屏（释放复制对象）。
    pub(crate) fn stop(&mut self) {
        tracing::info!("DXGI 桌面复制已停止");
    }
}

/// 在指定适配器上创建 D3D11 设备。
fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext), CapError> {
    let feature_levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
        D3D_FEATURE_LEVEL_9_2,
        D3D_FEATURE_LEVEL_9_1,
    ];
    // `D3D11CreateDevice` 只接受 `IDXGIAdapter`，这里显式转一次。
    let adapter: IDXGIAdapter = adapter.cast().map_err(|err| {
        tracing::error!(error = %err, "获取 DXGI 适配器接口失败");
        CapError::DeviceLost
    })?;
    let mut device = None;
    let mut device_context = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    // SAFETY: 适配器指针有效，特性级别数组有效，输出参数都是合法的可选项。
    let created = unsafe {
        windows::Win32::Graphics::Direct3D11::D3D11CreateDevice(
            Some(&adapter),
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut device_context),
        )
    };
    created.map_err(|err| {
        tracing::error!(error = %err, "创建 D3D11 设备失败");
        CapError::DeviceLost
    })?;
    match (device, device_context) {
        (Some(device), Some(device_context)) => Ok((device, device_context)),
        _ => {
            tracing::error!("D3D11CreateDevice 没有返回设备");
            Err(CapError::DeviceLost)
        }
    }
}

/// 读取显示器在虚拟桌面上的矩形。
fn monitor_rect(monitor: &Monitor) -> Result<RECT, CapError> {
    let hmonitor = HMONITOR(monitor.as_raw_hmonitor());
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(32),
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    // SAFETY: `info` 是合法的输出结构，`hmonitor` 来自有效的显示器枚举。
    let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info) };
    if !ok.as_bool() {
        tracing::error!("读取显示器矩形失败");
        return Err(CapError::NoDisplay);
    }
    Ok(info.rcMonitor)
}

/// 比较 DXGI 输出的设备名与目标显示器的设备名。
fn device_name_matches(device_name: &[u16; 32], target: &str) -> bool {
    let end = device_name
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(device_name.len());
    let name = String::from_utf16_lossy(&device_name[..end]);
    name.eq_ignore_ascii_case(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_name_comparison_is_case_insensitive() {
        // Windows 的设备名形如 `\\.\DISPLAY1`。
        let name: Vec<u16> = "\\\\.\\DISPLAY1"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut buffer = [0u16; 32];
        buffer[..name.len()].copy_from_slice(&name);
        assert!(device_name_matches(&buffer, "\\\\.\\DISPLAY1"));
        assert!(!device_name_matches(&buffer, "\\\\.\\DISPLAY2"));
    }

    /// 真机冒烟测试：真的用 DXGI 桌面复制抓一帧。
    ///
    /// 默认 `#[ignore]`：CI / 无桌面会话的环境里抓屏必然失败。
    /// 本机验证方式：
    ///
    /// ```text
    /// cargo test -p beam-capture --all-features dxgi_real_capture -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "需要真实显示器与交互式桌面会话"]
    fn dxgi_real_capture_produces_a_frame() {
        use crate::source::{CaptureBackend, CaptureConfig, CaptureSource, WgcCaptureSource};

        let mut source = WgcCaptureSource::new();
        let cfg = CaptureConfig {
            backend: CaptureBackend::Dxgi,
            display_id: None,
            min_update_interval: None,
            target_fps: 60,
        };
        source.start(cfg).expect("DXGI 桌面复制应当能启动");
        assert_eq!(source.backend_kind(), Some("dxgi"));

        let mut captured = None;
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if let Some(frame) = source.latest() {
                captured = Some(frame);
                break;
            }
        }
        source.stop();

        let frame = captured.expect("应当在 3 秒内抓到至少一帧");
        let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` 是合法的输出参数。
        unsafe { frame.tex.GetDesc(&mut desc) };
        assert!(desc.Width > 0, "纹理宽度应当有效");
        assert!(desc.Height > 0, "纹理高度应当有效");
        tracing::info!(
            width = desc.Width,
            height = desc.Height,
            ts_us = frame.ts_us,
            "DXGI 真机抓屏成功"
        );
    }
}
