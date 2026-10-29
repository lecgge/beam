# core-renderer — Surface 与帧节奏

> 技术设计文档 **§3.12** 的 `:core-renderer` 行：
> *职责*：Surface 管理、`Surface.setFrameRate`、帧节奏；
> *对外接口*：`FramePacer`；
> *禁止*：—。

## 职责

* 通过 `Surface.setFrameRate` 把视频表面的目标刷新率提示给合成器 / 显示驱动
  （Android 11 / API 30 起），这是 Android 端对齐目标刷新率 vsync 的前提；
* 提供帧节奏控制器，丢弃迟到帧、跳过乱序帧，避免延迟累积。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `FramePacer` | 帧节奏控制器；`decide(ptsUs, arriveUs): PacerDecision` |
| `PacerDecision` | `Present` / `Drop(lateByUs)` / `Skip(reason)` / `Wait(dueInUs)` |
| `SurfaceFrameRate` | `apply(surface, fps)` / `applyFixedSource(surface, fps)` |
| `SurfaceSource` | 表面来源抽象（`fun interface`） |

## 禁止事项落实

| 约束 | 落实方式 |
|---|---|
| 不做解码 | 不依赖 `:core-decoder`；本模块只算节拍，不碰码流 |
| 不做网络 | 不依赖 `:core-net` |

## 线程模型

`FramePacer` 非线程安全，由帧回调线程单线程调用。

## 口径说明（重要）

`ptsUs` 来自**主机**的单调时钟（§6 约定），与客户端本地时钟**不同源**，
两者不能直接相减。因此：

* 节拍由 `arriveUs`（帧到达客户端的单调时刻）驱动；
* `ptsUs` 只用于**顺序校验**（丢弃乱序 / 重复帧）。

## 测试

```bash
./gradlew :core-renderer:test
```

`FramePacerTest` 覆盖：首帧上屏、按时上屏、迟到丢帧、提前等待、乱序跳过、重置、帧间隔计算。
