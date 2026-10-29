# core-audio — Opus 解码与音频输出

> 技术设计文档 **§3.12** 的 `:core-audio` 行：
> *职责*：Opus 解码 + AAudio；
> *对外接口*：`AudioPlayer`；
> *禁止*：—。

## 职责

* 解码主机下发的 Opus 包（§3.7：48 kHz、5 ms 一帧）；
* 把 PCM 输出到设备。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `AudioPlayer` | `start(format)` / `enqueue(packet, ptsUs)` / `stop()` / `release()` |
| `PcmAudioFormat` | 采样率 / 声道 / 每采样字节数（默认 48 kHz / 2ch / 16 bit） |
| `LowLatencyAudioPlayer` | 默认实现：MediaCodec(`audio/opus`) → PCM16 → 低延迟输出 |
| `PcmRingBuffer` | 单生产者单消费者 PCM 环形缓冲（纯逻辑，可单测） |

## 关于 AAudio（重要偏差说明）

**AAudio 在公开 Android SDK 中没有 Java/Kotlin 绑定**：`AAudio.h` 只随 NDK 分发，
`android.jar` 里不存在 `AAudio` / `AAudioStream` 类（已用 `javap` 核对
`platforms/android-36/android.jar`）。因此本模块的 Java 侧走
`AudioTrack` 的低延迟路径：

* `AudioTrack.PERFORMANCE_MODE_LOW_LATENCY`（API 26+）；
* `AudioAttributes.USAGE_GAME` + `CONTENT_TYPE_MOVIE`；
* `MODE_STREAM` + 小缓冲。

该组合在系统内部路由到与 AAudio 相同的原生 fast mixer 快速通道
（`AudioTrack` 的低延迟模式即建立其上），延迟量级一致。

> 若后续需要直接持有 `AAudioStream`（自定义回调 / 独占流协商），应在
> `:core-audio` 内补一个 NDK/JNI 薄封装（CMake + `AAudio.h`），
> Kotlin 侧的 `AudioPlayer` 接口无需改动。

## 禁止事项落实

| 约束 | 落实方式 |
|---|---|
| 不做混音 / 回声消除 | 本模块只解码与直通输出，不做任何混音处理（§3.7 边界） |
| 不做网络 | 音频包由 `:core-net` 的 `Session.audioPackets` 下发，本模块不碰网络 |

## 线程模型

* `enqueue`：音频包投递线程（同步解码；Opus 帧只有 5 ms，够快）；
* 输出：同一个投递线程写 `AudioTrack`（阻塞写，缓冲满时自然背压）；
* `stop` / `release`：任意线程。

## 测试

```bash
./gradlew :core-audio:test
```

`PcmRingBufferTest` 覆盖：读写往返、空间不足截断、环形回绕、清空、零长度。
