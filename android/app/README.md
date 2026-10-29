# app — 应用外壳

> 技术设计文档 **§3.12** 的 `:app` 行：
> *职责*：导航、主题、权限、TV 入口；
> *对外接口*：—；
> *禁止*：**业务逻辑**。

## 职责

* 导航：5 个页面（主机列表 / 配对 / 串流 / 设置 / 性能），与原型
  `docs/prototype/interactive.html` 的 Android 端一一对应；
* 主题：深浅两套 Material3 配色（产品方案 §16）；
* 权限：按需申请（相机 / 通知 / 前台服务 / 局域网）；
* TV 入口：`TvMainActivity`（`LEANBACK_LAUNCHER`），与手机端共用同一套 UI。

## 目录

| 路径 | 说明 |
|---|---|
| `BeamApplication.kt` | 应用入口，创建 `AppContainer` |
| `AppContainer.kt` | 手动依赖注入（不使用 DI 框架） |
| `BeamRoot.kt` | 主题 + 导航的根 Composable |
| `BeamNavHost.kt` | 路由表与 5 个页面的装配 |
| `BeamPermissions.kt` | 权限清单与按需申请封装 |
| `theme/BeamTheme.kt` | 深浅色 `ColorScheme` 与 `Typography` |
| `src/debug/.../perf/PerfEntry.kt` | debug 性能面板入口（引用 `:feature-perf`） |
| `src/release/.../perf/PerfEntry.kt` | release 空实现（**不引用** `:feature-perf`） |

## Kotlin 对外接口

本模块不对外提供 Kotlin API（§3.12 表中为「—」），只承载装配。

## 依赖注入

**手动构造函数注入**：没有 Hilt / Koin / Dagger。

```
AppContainer
 ├─ DataStore ── SettingsRepo ──┐
 │            └─ HostRepo       │
 └─ NativeBeamBridge → BeamClient
```

每个 `feature-*` 的 ViewModel 都提供 `companion object { fun factory(...) }`
（一个手写的 `ViewModelProvider.Factory`），由 `BeamNavHost` 显式构造并传入。

## 禁止事项落实

| §3.12 禁止项 | 落实方式 |
|---|---|
| 业务逻辑 | `app` 只有装配与主题；页面逻辑在各 `:feature-*` 的 ViewModel 里；能力在各 `:core-*` 里 |
| 直接调 JNI | `app` 只依赖 `:core-net` 的 `BeamClient` **接口**；`System.loadLibrary` 只在 `NativeBeamBridge` 内 |
| release 依赖 feature-perf | `:app` 只用 `debugImplementation(project(":feature-perf"))`；`src/release` 里的 `PerfEntry` 是空实现 |

## 权限（按需声明）

| 权限 | 何时申请 | 位置 |
|---|---|---|
| `CAMERA` | 进入配对页 | `BeamNavHost` 的 `PAIRING` 路由 |
| `POST_NOTIFICATIONS` | 首次启动 | `BeamRoot` → `RequestNotificationPermissionOnFirstLaunch` |
| `FOREGROUND_SERVICE` / `FOREGROUND_SERVICE_MEDIA_PLAYBACK` | 声明式 | `AndroidManifest.xml` |
| `NEARBY_WIFI_DEVICES` / `CHANGE_WIFI_MULTICAST_STATE` | 声明式 | `AndroidManifest.xml`（mDNS 组播锁） |
| `ACCESS_FINE_LOCATION`（≤ API 32） | 声明式 | Wi-Fi 扫描需要 |

## TV 入口

* `AndroidManifest.xml` 中 `android.software.leanback` 声明为 `required="false"`，
  同一个 APK 同时支持手机与 TV；
* `TvMainActivity` 带 `LEANBACK_LAUNCHER` 分类，横屏；
* TV 端遥控器输入映射在 `:core-input` 的 `DpadDirection`。

## 构建

```bash
./gradlew :app:assembleDebug
./gradlew :app:assembleRelease
```

`release` 开启 R8（`isMinifyEnabled = true` + `isShrinkResources = true`），
`proguard-rules.pro` 里保留 uniffi 绑定与 `FrameSink` 实现的签名。
