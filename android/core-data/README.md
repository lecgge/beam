# core-data — 设置与信任主机

> 技术设计文档 **§3.12** 的 `:core-data` 行：
> *职责*：DataStore 设置、信任主机；
> *对外接口*：`SettingsRepo`、`HostRepo`；
> *禁止*：—。

## 职责

* 用 Jetpack DataStore Preferences 持久化串流设置（画质 / 帧率 / 编码 / FEC / 解码器偏好 / 统计浮层开关 / 主题）；
* 持久化**已配对主机与固定指纹**（信任库）。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `SettingsRepo` | `settings: Flow<BeamSettings>` / `update { }` / `reset()` |
| `HostRepo` | `hosts: Flow<List<TrustedHost>>` / `upsert` / `remove` / `find` / `isTrusted` / `clear` |
| `BeamSettings` | 设置模型；`toStreamCfg()` 转成 `:core-net` 的 `StreamCfg` |
| `DecoderPreference` | `AUTO` / `LOW_LATENCY` / `SYSTEM` |
| `ThemeMode` | `SYSTEM` / `LIGHT` / `DARK` |
| `TrustedHost` | 已信任主机（`Peer` + 配对时间 + 最近连接时间） |
| `PeerCodec` | 信任库的编解码（纯逻辑，可单测） |
| `dev.beam.core.data.ui.BeamTokens` | 设计令牌（产品方案 §16） |
| `dev.beam.core.data.perf.PerfUiState` 等 | 性能面板共享模型（见下） |

## 为什么设计令牌与 perf 模型放在这里

技术设计文档 §2 固定了 Android 端的 12 个模块，其中**没有 `:core-ui`**：

* **设计令牌**（颜色 / 间距 / 圆角 / 字号）是纯数据，需要被 `:feature-*` 与 `:app`
  同时引用，`:core-data` 是所有界面层模块的公共下层，因此放在
  `dev.beam.core.data.ui`；Material3 `ColorScheme` / `Typography` 的组装属于
  `:app`（§3.12 `:app` 职责含「主题」），见 `app/.../theme/BeamTheme.kt`。
* **性能面板的共享模型**必须同时被 debug 与 release 变体看到（`:app` 的
  `PerfEntry` 在两个变体里签名一致），而 `:feature-perf` 只在 debug 被依赖，
  因此模型放在 `:core-data`，实现留在 `:feature-perf`。

## 禁止事项落实

| 约束 | 落实方式 |
|---|---|
| 不做 UI 业务逻辑 | 本模块只有仓库与模型，无 Compose 界面代码（只有一个 `dp/sp/Color` 令牌对象） |
| 不直接访问网络 | 只读写 DataStore；Rust 侧由 `:core-net` 负责 |

## 安全前提（产品方案 §8）

扫码载荷在局域网上是明文，**主机端必须弹窗人工确认后才会写入信任库**。
客户端侧的写入只发生在用户主动完成配对（`PairingViewModel.onPaired`）之后。

## 线程模型

* `settings` / `hosts`：`Flow`，冷流，DataStore 内部在 IO 调度器上读取；
* `update` / `upsert` / `remove`：`suspend`。

## 测试

```bash
./gradlew :core-data:test
```

* `PeerCodecTest`：编解码往返、字段分隔符转义、非法记录、信任库整体编解码；
* `SettingsRepoTest`：默认值、更新持久化、reset、设置→`StreamCfg` 转换、信任库写入/查找/去重/删除。
