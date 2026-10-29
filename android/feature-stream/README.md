# feature-stream — 串流页

> 技术设计文档 **§3.12** 的 `:feature-stream` 行：
> *职责*：SurfaceView、悬浮工具栏、统计浮层；
> *对外接口*：`StreamScreen`；
> *禁止*：**解码细节**。

## 职责

* 用 `SurfaceView` 承载解码输出；
* 右上角**悬浮工具栏**（展开 / 收起、统计浮层开关、断开）；
* 左上角**统计浮层**，消费 `session://stats`（500 ms）；
* **输入接线**：把 Compose 的触控 / 按键 / 手柄事件交给 `:core-input` 映射，
  再经 `:core-net` 回传主机。

## 输入接线

| 环节 | 位置 |
|---|---|
| 采集 Modifier | `:core-input` 的 `beamTouchInput` / `beamKeyInput` / `BeamGamepadInput` |
| 映射 | `:core-input` 的 `InputMapper` / `AndroidKeys` / `GamepadMotion` |
| 接线 | 本模块：`StreamScreen` 挂 Modifier，`StreamViewModel.sendInput` 交给会话 |
| 传输 | `:core-net` 的 `Session.sendInput`（反射到 Rust 侧） |

本模块**只做编排**：不碰 `MediaCodec`，也不碰协议字段。

> **当前缺口**：Rust 侧 uniffi 导出面尚未提供 `Session::send_input`，
> 因此 `sendInput` 目前会失败；失败原因会显示在画面左下角
> （`StreamUiState.inputError`），不会静默吞掉。

## 消费的 §3.11 事件语义

| 事件 | 频率 | 本模块的处理 |
|---|---|---|
| `session://stats` | 500 ms | 写入 `StreamUiState.stats`，统计浮层渲染端到端 / 解码 / 显示耗时、分辨率、编码、丢包与 FEC 恢复率 |
| `session://reconnecting` | 事件 | `phase = RECONNECTING`，画面保留最后一帧并叠加「正在重连…」遮罩 |
| `session://disconnected` | 事件 | `phase = DISCONNECTED`，显示断开文案 |
| `session://state-changed` | 事件 | 驱动会话状态机 |
| 能力协商完成 | 事件 | 用主机的最终配置刷新 `resolutionLabel` / `codecLabel` |
| `perf://tick` | debug 250 ms | **不在本模块消费**——它只存在于 debug，由 `:feature-perf` 消费（见下） |

> `perf://tick` 若在本模块引用 `:feature-perf`，会把 `feature-perf` 拉进
> release 依赖图，违反 §3.12 / §3.13 的 release 门禁。因此 debug 统计走
> `:feature-perf` 的 `PerfRecorder`（订阅 `:core-net` 的 `PerfTickSource`），
> 结果写入 `:core-data` 的 `PerfSnapshotStore`，由 `:app` 的 debug 变体渲染。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `StreamViewModel` | `state: StateFlow<StreamUiState>`；`connect` / `onSurfaceAvailable` / `onSurfaceDestroyed` / `disconnect` / `toggleOverlay` / `toggleToolbar` / `sendInput` / `requestIdr` |
| `StreamUiState` | `phase` / `peer` / `stats` / `message` / `overlayVisible` / `toolbarVisible` / `resolutionLabel` / `codecLabel` / `droppedFrames` / `inputError` |
| `StreamPhase` | `IDLE` / `CONNECTING` / `STREAMING` / `RECONNECTING` / `DISCONNECTED` / `ERROR` |
| `StreamScreen` | 纯展示 Composable（状态 + 回调） |

## 禁止事项落实

| §3.12 禁止项 | 落实方式 |
|---|---|
| 解码细节 | 本模块**不**接触 `MediaCodec`：解码器经 `:core-decoder` 的 `VideoDecoderFactory` 创建，流格式也由该工厂构造（只传编码名）；节拍由 `:core-renderer` 的 `FramePacer` 提供 |
| 直接调 JNI | 帧来自 `:core-net` 的 `FrameSink` 回调 |

## 线程模型

* 帧回调发生在 Rust 派发的专用线程：`StreamSink.onFrame` 在该线程里
  按 `FramePacer` 决策后调用 `decoder.queue(...)`；
* `FrameSink` 契约规定缓冲在回调返回后失效，因此「等待节拍」分支会**先拷贝**
  再延迟提交（该分支罕见，拷贝成本可接受）；
* UI 状态更新在 `viewModelScope`（主线程），由 `StateFlow` 驱动重组。

## 帧节奏口径

`FramePacer` 的节拍由**到达时刻**驱动、`pts` 只做顺序校验，详见
`core-renderer/README.md`。
