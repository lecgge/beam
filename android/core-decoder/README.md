# core-decoder — MediaCodec 硬解

> 技术设计文档 **§3.12** 的 `:core-decoder` 行：
> *职责*：MediaCodec 异步模式、`KEY_LOW_LATENCY`、厂商 low_latency 解码器、output fence；
> *对外接口*：`interface VideoDecoder { fun start(surface: Surface, fmt: VideoFormat); fun queue(frame: ByteBuffer, ptsUs: Long, key: Boolean); fun stop() }`；
> *禁止*：**网络**。

## 职责

* 用 `MediaCodec` 异步模式把 H.264 / HEVC / AV1 码流解码到 `Surface`；
* 下发 `MediaFormat.KEY_LOW_LATENCY = 1`；
* 自动探测设备解码器，优先厂商 low-latency 硬解（§9 风险表「国产机 MediaCodec 差异」）；
* **输出栅栏（output fence）**：输出缓冲的索引、显示时间戳与「该不该上屏」由
  `OutputFence` 显式登记；渲染到 Surface 时由系统处理内部 fence（公开 SDK
  不暴露 `SyncFence`），不额外拷贝。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `VideoDecoder` | §3.12 规定的解码器接口（签名逐字一致） |
| `VideoFormat` | 流格式：分辨率 / 编码 / profile / level / 帧率 |
| `DecoderCodec` | `H264` / `HEVC` / `AV1`，带 `mime` |
| `MediaCodecVideoDecoder` | 默认实现 |
| `VideoDecoderFactory` | **工厂**：编码名 → 解码器 / `VideoFormat`（让 `:feature-stream` 不接触解码细节） |
| `OutputFence` | 输出栅栏：在途缓冲索引 / 显示时间戳 / 已渲染与已跳过计数 |
| `DecoderSelector` | 解码器选型策略（纯逻辑，可单测） |
| `DecoderCandidate` | 解码器候选（纯数据，可单测） |
| `DeviceDecoders` | 设备解码器枚举 |

## 禁止事项落实

| §3.12 禁止项 | 落实方式 |
|---|---|
| 依赖网络 | `build.gradle.kts` **不依赖** `:core-net`；本模块无任何 socket / Rust 相关引用 |
| 帧数据在网络层流转 | 输入是调用方直接交来的 `ByteBuffer`；模块内不做任何传输 |

## 线程模型

* `start` / `queue` / `stop`：调用方线程（`:feature-stream` 的帧回调线程）；
* `onInputBufferAvailable` / `onOutputBufferAvailable`：内部 `HandlerThread`；
* `queue` 不阻塞等待输入缓冲：帧先进内部队列，由回调线程取走。

## 关键实现说明

### 能力探测走反射

`MediaCodecInfo` 的能力结构在 **Android 16（API 36）** 发生改名：

* `MediaCodecInfo.Capabilities` → `MediaCodecInfo.CodecCapabilities`；
* `VideoCapabilities.getSupportedWidths()` 从 `int[]` 改成 `Range<Integer>`。

本工程 `minSdk = 26`，直接引用新类型会在旧设备上 `NoClassDefFoundError`，
因此 `DeviceDecoders` 的能力探测**一律用反射**，对两种 API 形态都成立。

### 帧所有权

`VideoDecoder.queue` 的 `ByteBuffer` 在调用返回后即失效（与 `:core-net` 的
`FrameSink` 契约一致）。`MediaCodec.queueInputBuffer` 会自行拷贝输入数据，
因此可以同步提交而无需额外拷贝。

### 输出栅栏（output fence）

MediaCodec 的**公开 SDK 不暴露** Surface 输出的原始 fence —— 框架在
`onOutputBufferAvailable()` 回调之前就已经等过内部 fence。因此本模块把
输出侧**可观测、可测试**的那部分显式化：

* `OutputFence.begin(index, ptsUs, isCodecConfig)` 登记一次输出缓冲，
  并判断该缓冲**该不该上屏**；
* `OutputFence.complete(index)` 登记缓冲已交还 MediaCodec；
* `MediaCodecVideoDecoder.outputFence` 公开该对象，供上层统计在途索引、
  已渲染帧数与被跳过的缓冲数。

> **顺带修掉一个真实缺陷**：此前 `onOutputBufferAvailable` 完全忽略
> `MediaCodec.BufferInfo`，导致 **codec-config 缓冲（SPS/PPS）也被当成画面渲染上屏**。
> 现在带 `BUFFER_FLAG_CODEC_CONFIG` 的缓冲只交还、不渲染。

## 测试

```bash
./gradlew :core-decoder:test
```

`DecoderSelectorTest` 覆盖：低延迟优先、硬解优先、软解兜底、白名单识别、mime 取值。

`OutputFenceTest` 覆盖：普通缓冲渲染计数、codec-config 缓冲跳过计数、
交还后清空在途索引、索引错配忽略、重置、编码名到容器格式的解析与流格式构造。

## 性能预算（§3.12）

解码 T1 ≤ 8 ms、T2 ≤ 5 ms（Android 端不作为 T3 验收目标）。
