# feature-perf — 性能面板（仅 Debug）

> 技术设计文档 **§3.12** 的 `:feature-perf` 行：
> *职责*：性能面板（仅 `debugImplementation`）；
> *对外接口*：`PerfPanel`；
> *禁止*：**被 release 依赖**。

## 门控（§3.13 / §7）

* `:app` 用 `debugImplementation(project(":feature-perf"))` 引入本模块；
* **release 变体的依赖图里没有 `:feature-perf`**，因此 release APK 中不存在任何
  feature-perf 类；
* `:app` 的 `BuildConfig.PERF_STATS` 在 debug 为 `true`、release 为 `false`；
* `:app` 的 `PerfEntry` 在 `src/debug` 与 `src/release` 各有一份**同名同签名**实现：
  debug 引用 `:feature-perf`，release 是空实现。

### 可执行校验

```bash
./gradlew :app:assembleRelease
unzip -l app/build/outputs/apk/release/*.apk | grep -i "feature-perf\|perfstats" && exit 1 || exit 0
# R8 映射表同样检查
grep -i "feature-perf\|perfstats" app/build/outputs/mapping/release/mapping.txt && exit 1 || exit 0
```

## 职责

* 展示各阶段耗时（抓屏 / 颜色转换 / 编码 / 分包 / 网络 / 解码 / 渲染 / E2E 等）
  的均值与 P99；
* 展示帧率、码率、丢包率、FEC 恢复率、掉帧数、RTT、端到端延迟、分辨率、编码；
* 端到端延迟折线（虚线参考 16.7 ms = 60 fps 一帧）；
* 导出 CSV / JSON（§3.13 `Perf::export(fmt, path)`）。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `PerfPanel` | §3.12 规定的面板 Composable：`PerfPanel(state, onExport, modifier)` |
| `PerfRecorder` | 采集器：订阅 `session://stats` 与 `perf://tick`，写入 `PerfSnapshotStore` |
| `StageAccumulator` | 阶段耗时环形累加器（均值 / P99，纯逻辑，可单测） |

共享模型（`PerfUiState` / `PerfExportFormat` / `PerfSnapshotStore`）在
`:core-data` 的 `dev.beam.core.data.perf` 包里——因为 `:app` 的 debug / release
两个变体都需要看到同一份模型，而 `:feature-perf` 只在 debug 被依赖。

## 数据来源

| 来源 | 频率 | 说明 |
|---|---|---|
| `session://stats` | 500 ms | 客户端可观测统计 |
| `perf://tick` | debug 250 ms | Rust 侧各阶段耗时（`beam-perf` 的 `perf-stats` cargo feature，默认关闭） |

## 线程模型

`PerfRecorder` 在调用方给的 `CoroutineScope` 里收集两个 Flow；
采样与发布在协程上下文中完成，不阻塞 UI。

## 测试

```bash
./gradlew :feature-perf:test
```
