# feature-settings — 串流设置页

> 技术设计文档 **§3.12** 的 `:feature-settings` 行：
> *职责*：设置页；
> *对外接口*：—（§3.12 表中未指定 Kotlin 接口）；
> *禁止*：—。

## 职责

串流设置（产品方案 §7 `:feature-settings`）：画质、帧率、码率、编码、FEC 冗余、
解码器偏好、统计浮层开关、音频开关、主题。

页面对应原型 `docs/prototype/interactive.html` 的 Android 端「串流设置」页。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `SettingsViewModel` | `state: StateFlow<SettingsUiState>`；`setResolution` / `setFps` / `setCodec` / `setBitrate` / `setFecRatio` / `setAudioEnabled` / `setOverlayEnabled` / `setDecoderPreference` / `setThemeMode` / `reset` |
| `SettingsUiState` | `settings: BeamSettings` / `loaded` |
| `SettingsScreen` | 纯展示 Composable（状态 + 回调） |

## 数据流

单一数据源：所有写操作都落到 `:core-data` 的 `SettingsRepo`（DataStore），
再由 `settings` Flow 回流刷新 UI，**不在 ViewModel 里维护第二份状态**。

## 禁止事项落实

| 约束 | 落实方式 |
|---|---|
| UI 与逻辑分离 | `SettingsScreen` 只渲染状态并回调；持久化在 ViewModel → `SettingsRepo` |
| 不直接调 JNI | 本模块完全不依赖 `:core-net` 的原生能力，只用其协议模型（`VideoCodec`） |

## 线程模型

* `settingsRepo.settings` 在 `viewModelScope` 里收集；
* 写操作 `suspend`，由 DataStore 内部调度器处理。
