# core-input — 输入采集与映射

> 技术设计文档 **§3.12** 的 `:core-input` 行：
> *职责*：触控/手柄/遥控映射为 `InputEvent`；
> *对外接口*：`InputMapper`；
> *禁止*：—。

## 职责

把 Android 侧的设备输入（触控 / 物理键盘 / 实体手柄 / 遥控器 D-pad）映射成
`beam-proto::InputEvent` 的 Kotlin 模型，并回传主机。

**字段与 `:core-net` 的 `InputEvent.kt` 完全对齐** —— `:core-net` 负责线上协议
模型与序列化，本模块只做「设备事件 → 协议事件」的映射，**不定义新的事件类型**。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `InputMapper` | 映射器：`touch` / `key` / `dpad` / `mouseMove` / `mouseButton` / `mouseWheel` / `gamepadButton` / `gamepadAxis` |
| `InputMapperConfig` | 摇杆死区、是否归一化、是否反转 Y |
| `DpadDirection` | 遥控器 / D-pad 方向 → 方向键键码 |
| `MotionActions` | `MotionEvent` action → `TouchAction` |
| `AndroidKeys` | Android `KeyEvent.keyCode` → 协议键码 / 手柄按键 |
| `GamepadMotion` | Android `MotionEvent` 模拟轴 → 协议轴 |
| `Modifier.beamTouchInput` | Compose 触控采集（`pointerInput`） |
| `Modifier.beamKeyInput` | Compose 按键 / 遥控 / 手柄按键采集（`onKeyEvent`） |
| `BeamGamepadInput` | 手柄模拟轴采集（Android generic motion 通道） |
| `InputClock` | 单调时钟微秒（协议时间戳口径） |

## 采集 → 回传链路

```
Compose 触控/按键/手柄
        │  （本模块：Android 事件 → InputEvent）
        ▼
InputEvent (`:core-net` 的协议模型)
        │  （`:core-net`：序列化回传）
        ▼
Session.sendInput(event) → Rust 主机
```

* **采集**在本模块（`Modifier.beamTouchInput` / `beamKeyInput` / `BeamGamepadInput`）；
* **映射**在本模块（`InputMapper` / `AndroidKeys` / `GamepadMotion`）；
* **回传**在 `:core-net`（`Session.sendInput`）；
* **接线**在 `:feature-stream`（`StreamScreen` 把 Modifier 挂到画面上，
  `StreamViewModel.sendInput` 交给会话）。

> **当前缺口**：Rust 侧 uniffi 导出面（`crates/beam-ffi/src/ffi.rs`）尚未导出
> `InputEvent`，`Session` 也只有 `state()` / `stop()`，
> 因此 `Session.sendInput` 目前会抛 `BeamNativeUnavailableException`。
> 上面的链路已就位，补齐 Rust 导出面后即可打通。

## 映射口径

| 设备输入 | 协议事件 | 备注 |
|---|---|---|
| 触控 | `InputEvent.Touch` | 同时给出像素坐标与**归一化坐标**（主机侧据此换算回绝对坐标） |
| 物理键盘 / 遥控器 | `InputEvent.Key` | 键名用 DOM 风格键码（`ArrowUp` / `Enter` / `KeyA`） |
| 鼠标 | `InputEvent.MouseMove` / `MouseButton` / `MouseWheel` | 相对位移，像素 |
| 手柄按键 | `InputEvent.GamepadButton` | Xbox 布局（`GamepadButton` 枚举） |
| 手柄摇杆 / 扳机 | `InputEvent.GamepadAxis` | 死区内**不下发**事件 |

## 禁止事项落实

| 约束 | 落实方式 |
|---|---|
| 不做网络 | 本模块只产出事件对象；上行由 `:core-net` 的 `Session.sendInput` 负责 |
| 不重复定义协议模型 | 直接复用 `:core-net` 的 `InputEvent` / `TouchAction` / `GamepadButton` / `GamepadAxis` |

## 线程模型

无状态（除配置），可在任意线程调用；建议在主线程或输入回调线程调用。

## 测试

```bash
./gradlew :core-input:test
```

`InputMapperTest` 覆盖：触控归一化与边界、Y 轴反转、摇杆死区与线性重映射、
D-pad 映射、键鼠事件字段对齐。

`AndroidKeysTest` 覆盖：方向键 / 确认键 / 字母数字键的协议码、无法识别的按键、
手柄按键映射、Android 轴索引到协议轴的映射、摇杆死区。
