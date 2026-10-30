# core-net — Rust（uniffi）接入层

> 对应技术设计文档 **§3.12** 的 `:core-net` 行：
> *职责*：uniffi 绑定 `BeamClient`、回调转 Flow；
> *对外接口*：`interface BeamClient { suspend fun connect(peer: Peer, cfg: StreamCfg): Session }`；
> *禁止*：—（本模块是**唯一**允许触碰 JNI 的地方）。

本模块**不含任何解码/渲染/输入采集逻辑**：帧数据只是从 Rust 搬到 Kotlin，
真正的解码在 `:core-decoder`，上屏在 `:core-renderer`，输入采集在 `:core-input`。

---

## 1. 目录内容

| 路径 | 说明 |
|---|---|
| `src/main/java/dev/beam/core/net/BeamClient.kt` | 对外接口（`discover` / `pairQr` / `pairPin` / `connect`） |
| `.../Session.kt`、`SessionEvent.kt`、`SessionStats` | 会话接口与控制面事件（§3.11 events 语义） |
| `.../Peer.kt`、`StreamCfg.kt`、`Models.kt` | 线上协议模型 |
| `.../InputEvent.kt` | 输入事件模型（与 `beam-proto::InputEvent` 对齐） |
| `.../FrameSink.kt`、`PooledFrameSink.kt` | 帧回调接口（热路径 DirectByteBuffer） |
| `.../NativeBeamBridge.kt` | **反射适配层**：调用 uniffi 生成类 |
| `.../NativeMappings.kt` | 生成物 → 模型的映射（纯函数，可单测） |
| `.../Reflect.kt` | 反射工具 + `BeamNativeUnavailableException` |
| `.../buffer/ByteBufferPool.kt` | DirectByteBuffer 池（§6 内存所有权约定） |
| `src/main/jniLibs/arm64-v8a/`、`src/main/jniLibs/x86_64/` | 原生库输出目录（**不提交 `.so`**） |

---

## 2. 生成 uniffi Kotlin 绑定与原生库

### 2.1 一条命令（推荐）

Gradle 里已经接好两个任务：

```powershell
# 交叉编译 libbeam_ffi.so（arm64-v8a + x86_64）
.\gradlew.bat :core-net:cargoNdkBuild

# 用 uniffi-bindgen 生成 Kotlin 绑定（读刚编出来的 .so）
.\gradlew.bat :core-net:generateUniffiBindings
```

需要的环境：

| 依赖 | 说明 |
|---|---|
| `cargo` + `cargo-ndk` | `cargo install cargo-ndk` |
| Android NDK | `ANDROID_NDK_HOME` 环境变量 / `-Pbeam.ndk.home=<路径>` / `local.properties` 的 `sdk.dir\ndk\<版本>` |
| `uniffi-bindgen` | PATH 上的 `uniffi-bindgen`，或 `-Pbeam.uniffi.bindgen=<可执行文件路径>` |
| `FFMPEG_DIR` / `LIBCLANG_PATH` | 只有打开 Rust 侧可选的 `ffmpeg` feature 时才需要（`Exec` 默认继承当前进程环境变量） |

### 2.2 接进常规构建（可选）

```powershell
.\gradlew.bat -Pbeam.native=true :core-net:assembleDebug
```

`-Pbeam.native=true` 会让 `compile*Kotlin` 依赖 `generateUniffiBindings`、
`merge*NativeLibs` 依赖 `cargoNdkBuild`。

**默认（不加这个开关）不参与构建**：本机没有 cargo / NDK / uniffi-bindgen 时，
`assembleDebug` 仍必须能通过 —— 降级路径（`BeamNativeUnavailableException` →
`UnavailableBeamClient` → App 仍可启动）不能被破坏。

### 2.3 产物落点

| 产物 | 落点 | 是否入库 |
|---|---|---|
| `libbeam_ffi.so` | `src/main/jniLibs/{arm64-v8a,x86_64}/` | **否**（`.gitignore`） |
| uniffi Kotlin 绑定 | `build/generated/uniffi/uniffi/beam_ffi/beam_ffi.kt` | **否**（构建目录） |

生成的 Kotlin 绑定通过 `sourceSets` 挂进编译（`build.gradle.kts`），
因此**不需要**把生成物放进源码目录。若确实要手工生成一份放进
`src/main/java/dev/beam/core/net/uniffi/`，该目录已被 `.gitignore` 忽略，
且 `build.gradle.kts` 会优先使用它（避免重复类）。

> 生成物的 **package 是 `uniffi.beam_ffi`**（不是目录名 `uniffi`），
> `NativeBeamBridge` 的候选类名列表里已经包含这个前缀，因此两边对得上。

### 2.4 JNA 依赖

uniffi 生成的 Kotlin 绑定依赖 `com.sun.jna.*`，并且用到
`com.sun.jna.internal.Cleaner` —— 因此 **JNA 必须 ≥ 5.9.0**
（当前锁定 `5.14.0`）。该依赖只在真的要编译生成物时才引入
（`-Pbeam.native=true` 或生成目录已存在），避免给降级构建塞无用依赖。

---

## 3. 生成物缺席时的行为

本工程**不依赖**生成物即可编译（所有交互走反射）。运行时缺生成物会抛出
**明确可读**的异常，而不是 `ClassNotFoundException` 或静默失败：

| 缺失物 | 异常 | 提示内容 |
|---|---|---|
| `libbeam_ffi.so` | `BeamNativeUnavailableException` | 「无法加载原生库 libbeam_ffi.so … 请用 cargo-ndk 构建并把产物复制到 jniLibs/arm64-v8a 与 jniLibs/x86_64」 |
| `BeamClient` 类 | `BeamNativeUnavailableException` | 「未找到 uniffi 生成的 BeamClient 类（尝试过：…）」 |
| `BeamClient.browse` 等方法 | `BeamNativeUnavailableException` | 「uniffi 生成的 BeamClient 缺少方法 browse …」 |
| `Session.send_input` | `BeamNativeUnavailableException` | 「uniffi 会话句柄缺少 sendInput 方法 … Rust 侧需要在 crates/beam-ffi/src/ffi.rs 的 Session 导出面补 `pub fn send_input(&self, event: InputEvent)`」 |

`NativeBeamBridge.isAvailable()` 供 UI 区分「so 没构建」与「真的没找到主机」。

---

## 4. uniffi 导出面约定（**实测形状**）

下表取自 `crates/beam-ffi/src/ffi.rs` 的真实导出面，并用
`uniffi-bindgen 0.29.5` 生成的 Kotlin 绑定核对过。

| 生成物 | 实际形状 |
|---|---|
| `BeamClient` | `class BeamClient(config: BeamClientConfig)`；方法 `browse(listener)` / `pairQr(payload): Peer` / `pairPin(peer, pin): Peer` / `connect(peer, cfg, sink): Session` |
| `Session` | `class Session`；方法 `state(): SessionState` / `stop()` / **`reconnecting(): ReconnectingInfo?`** / **`summary(): SessionSummary`** |
| `FrameSink` | `interface FrameSink { onFrame(data: ByteArray, ptsUs: ULong, captureTsUs: ULong, key: Boolean); onEvent(event: SessionEvent) }` |
| `DiscoveryListener` | `interface DiscoveryListener { onHost(host: DiscoveredHost); onHostLost(fullname: String) }` |
| `SessionEvent` | `sealed class`；变体 `StateChanged(state)` / `Paired(fingerprint)` / `Negotiated(config)` / **`Stats(stats)`** / `Error(code, message, retryable)` / `Closed` / `FramesDropped(count, code)` |
| `SessionStats` | `sessionId: ULong` / `fps: UInt` / `bitrateBps: UInt` / `lossPpm: UInt` / `rttUs: UInt` / `decodeUs: UInt` / `renderUs: UInt` / `e2eUs: UInt` / `droppedFrames: UInt` |
| `ReconnectingInfo` | `elapsedMs: ULong` / `keepLastFrame: Boolean` —— **查询式**，不在重连态时返回 `null`（§4.1） |
| `SessionSummary` | `sessionId: ULong` / `durationMs: ULong` / `frames: ULong` / `droppedFrames: ULong` / `reconnects: UInt` / `rttUs: UInt` / `lossPpm: UInt` / `fps: UInt` / `bitrateBps: UInt` / `state: SessionState` —— **查询式**，会话结束后依然可查（§3.9） |
| `DiscoveredHost` | `id` / `name` / `version` / `fingerprint` / `port: UInt` / `address` / `fullname` —— **地址与端口是分开的两个字段** |
| `Peer` | `id` / `name` / `address` / `fingerprint` / `platform` / `trusted` |
| `StreamCfg` | `width` / `height` / `fps` / `codec: String` / `bitrateKbps` / `fecRatio` / `audioEnabled` |
| `BeamClientConfig` | `clientName` / `logLevel` / `fecRatio` / `jitterFrames` / `connectTimeoutMs` / `reconnectTimeoutMs` |
| `InputEvent` | **尚未导出**（Rust 侧没有该类型） |

### 4.1 三个必须记住的坑

1. **`SessionEvent.Stats` 是嵌套记录**：统计字段在 `stats: SessionStats` 上，
   不在变体本身。直接读变体会全读到 0 值。
2. **单位不同**：Rust 用整数最小单位（µs / ppm / bit·s⁻¹），
   Kotlin 侧模型用 ms / 百分比 / kbit·s⁻¹，换算集中在 `NativeMappings.mapStats`。
3. **Kotlin 会修饰 inline class 的 getter 名**：`val fps: UInt` 的 getter 在 JVM 上
   叫 `getFps-<hash>`，不是 `getFps`。只按精确名字查找方法会让**所有无符号整型字段
   读成 null** —— `Reflect.findMethodByName` 已容忍该后缀。

### 4.1.1 为什么重连与汇总是「查询」而不是「事件」

`SessionEvent` 在 `beam-session` / `beam-ffi` / 桌面端 `service_crates.rs` 各有一处
**穷尽匹配**；给它加一个变体会同时打破三处。因此导出面选择**查询式方法**：

| 查询 | 返回 | 语义 |
|---|---|---|
| `Session::reconnecting()` | `ReconnectingInfo?` | 不在重连态时是 `null`（§4.1） |
| `Session::summary()` | `SessionSummary` | 会话结束后依然可查（§3.9） |

Kotlin 侧对应 [Session.reconnecting] / [Session.summary]，
映射在 `NativeMappings.mapReconnectingInfo` / `NativeMappings.mapSessionSummary`。
`NativeSession` 用反射调用；生成物是旧版 `.so`（没有这两个方法）时返回 `null`，
不会崩。

### 4.2 `Result` 被 uniffi 提升

uniffi 会把 `Result<T, E>` 提升成「成功返回值 / 失败抛异常」：

* `pair_qr` 成功时**直接返回 `Peer` 记录**（不是 `Ok(Peer)` 包装）；
* `pair_pin` 成功时返回 `Unit`；
* 失败时抛 `BeamException` 的子类，其 `message` **不含错误码**
  （`PinFailed.message` 是空串），只能按**异常类名**映射
  （`InvalidPayload`→`E14001`、`PinFailed`→`E14002` …）。

record 的构造**按参数名**装配（`kotlin-reflect` 的 `callBy`），因此 Rust 字段
顺序变化不会破坏适配层。

---

## 5. 热路径：DirectByteBuffer 池化

§6 内存所有权约定：「JNI 热路径 DirectByteBuffer 由 Rust 侧池化，回调返回后归还」。

`NativeBeamBridge.deliverFrame` 的行为：

1. Rust 传入 `ByteBuffer` → 直接接管所有权；
2. Rust 传入 `ByteArray` → 从 `ByteBufferPool` 租一块直接内存并拷贝；
3. 消费方实现 `PooledFrameSink` → **所有权转移**给消费方，由其在解码完毕后
   `PooledFrame.close()` 归还；
4. 否则在 `onFrame` 返回后立即归还（回调期间缓冲有效，回调后失效）。

> MediaCodec 的 `queueInputBuffer` 会自行拷贝输入数据，因此在 `onFrame`
> 内同步 `queue()` 后缓冲即可安全归还，无需额外拷贝。

---

## 6. 控制面：回调 → Flow

Rust 侧的 `SessionEvent` 回调进入 `SessionBus.events`（`MutableSharedFlow`），
再派生为两个 Flow：

| 接口 | 来源 | 语义（§3.11） |
|---|---|---|
| `Session.events` | `SessionBus.events` | 全部控制面事件 |
| `Session.stats` | `SessionBus.events.filterIsInstance<Stats>()` | `session://stats`，500 ms |
| `Session.audioPackets` | `SessionBus.audio` | Opus 音频包（v1） |
| `Session.state` | `StateChanged` 事件驱动（建连时先问一次原生 `state()`） | §3.9 会话状态机 |

---

## 7. 线程模型

* Rust tokio 运行时在 `.so` 内；
* 帧回调发生在 Rust 派发的专用线程（**不在主线程**）；
* 控制面事件经 `MutableSharedFlow` 转成 Flow，UI 在主线程消费；
* 禁止在帧回调里做阻塞操作。

---

## 8. 禁止事项落实

| §3.12 禁止项 | 落实方式 |
|---|---|
| 其他模块直接调 JNI | `System.loadLibrary` / `Class.forName` 只出现在本模块的 `NativeBeamBridge` / `Reflect` |
| 本模块做解码/渲染 | 不依赖 `:core-decoder` / `:core-renderer`；帧只做透传 |
| 帧数据走 uniffi 对象拷贝 | 热路径用 `ByteBuffer`（DirectByteBuffer）+ 池化；控制面才走 uniffi |

---

## 9. 测试

```bash
./gradlew :core-net:test
```

`NativeMappingsTest` 覆盖：嵌套统计的映射与单位换算、统计事件字段非零、
协商结果补齐分辨率/帧率、状态/错误/关闭/协商事件的映射、
发现结果的 `host:port` 拼接、扫码与 PIN 配对的结果形状、错误码映射。

> 测试用 `dev.beam.core.net.fakes` 里的**替身类型**（与生成物同构），
> 因为生成物是构建产物且依赖 JNA 与 `.so`，无法在 JVM 单测里加载。
