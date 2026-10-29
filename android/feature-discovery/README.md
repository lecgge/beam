# feature-discovery — 主机列表

> 技术设计文档 **§3.12** 的 `:feature-discovery` 行：
> *职责*：主机列表、NSD 兜底；
> *对外接口*：`DiscoveryViewModel`；
> *禁止*：**直接调 JNI（经 core-net）**。

## 职责

* 展示局域网内的 Windows 主机列表（已配对 / 未配对 / 离线）；
* Rust 侧 mDNS（`beam-discovery`，mdns-sd）为主，Android `NsdManager` 为兜底
  （§3.4「Android 可选用 NsdManager 兜底（在 Kotlin 层）」）；
* 合并两个来源的发现结果，并按指纹标注信任状态。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `DiscoveryViewModel` | `state: StateFlow<DiscoveryUiState>`；`start()` / `stop()` / `refresh()` |
| `DiscoveryUiState` | `isScanning` / `hosts` / `error` / `nativeReady` |
| `HostRow` | 一行主机：peer、来源、是否信任、最后发现时间 |
| `HostScanner` | 扫描源接口（便于替换与测试） |
| `NsdHostScanner` | `NsdManager` 兜底实现 |
| `DiscoveryScreen` | 纯展示 Composable（参数为状态 + 回调） |

## 禁止事项落实

| §3.12 禁止项 | 落实方式 |
|---|---|
| 直接调 JNI | 本模块**只**依赖 `:core-net` 的 `BeamClient` 接口；`Class.forName` / `System.loadLibrary` 只存在于 `:core-net` 的 `NativeBeamBridge` |
| 业务逻辑进 UI | `DiscoveryScreen` 无状态，只渲染 `DiscoveryUiState` 并回调 `onHostClick` / `onPairClick` / `onRefresh` / `onOpenSettings` |

## 线程模型

* 发现流在 `viewModelScope`（`Dispatchers.Main.immediate`）里收集；
* `BeamClient.discover()` 的上游在 `:core-net` 内部完成线程切换；
* UI 只读 `StateFlow`。

## 降级行为

原生库尚未构建时（`nativeReady == false`），界面顶部显示明确提示：
「原生库尚未构建：请按 core-net/README.md 生成 uniffi 绑定与 libbeam_ffi.so 后重新编译。」
