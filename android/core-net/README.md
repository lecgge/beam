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
| `.../Reflect.kt` | 反射工具 + `BeamNativeUnavailableException` |
| `.../buffer/ByteBufferPool.kt` | DirectByteBuffer 池（§6 内存所有权约定） |
| `src/main/jniLibs/arm64-v8a/`、`src/main/jniLibs/x86_64/` | 原生库占位目录（**不放假 .so**） |

---

## 2. 生成 uniffi Kotlin 绑定

### 2.1 安装工具

```bash
cargo install uniffi-bindgen            # 生成 Kotlin 绑定
cargo install cargo-ndk                  # 交叉编译 .so（可选，也可用 cargo-ndk 命令行）
```

> `cargo install` 需要 crates.io 可达；网络受限时可改用
> `cargo install uniffi-bindgen --locked` 或从 release 下载预编译二进制。

### 2.2 生成 Kotlin 绑定

在仓库根目录（`crates/beam-ffi`）执行：

```bash
# 方式 A：uniffi-bindgen 直接绑定
uniffi-bindgen generate --library target/release/libbeam_ffi.so \
    --language kotlin --out-dir android/core-net/src/main/java/dev/beam/core/net/uniffi

# 方式 B：工程内 build.rs 自行生成（推荐，可离线复现）
#   crates/beam-ffi/build.rs 内调用 uniffi::generate_scaffolding("src/beam_ffi.udl")
```

生成物落在 `android/core-net/src/main/java/dev/beam/core/net/uniffi/`，包名
`dev.beam.core.net.uniffi`（与 `NativeBeamBridge` 的候选类名前缀一致）。

### 2.3 交叉编译 `libbeam_ffi.so`

```bash
cargo install cargo-ndk

# arm64-v8a（真机）
cargo ndk -t arm64-v8a -o android/core-net/src/main/jniLibs build --release -p beam-ffi

# x86_64（模拟器）
cargo ndk -t x86_64   -o android/core-net/src/main/jniLibs build --release -p beam-ffi
```

产物布局（AGP 会自动打进 APK 的 `lib/<abi>/`）：

```
core-net/src/main/jniLibs/
├─ arm64-v8a/libbeam_ffi.so
└─ x86_64/libbeam_ffi.so
```

> **不要把 `.so` 提交进仓库**：`.gitignore` 已忽略
> `core-net/src/main/jniLibs/*/libbeam_ffi.so`。
> 当前仓库里只有空目录占位（`.gitkeep`），**没有假 .so**。

### 2.4 在 Gradle 里接入（可选，推荐）

若希望 `assembleDebug` 自动跑 cargo-ndk，可在 `core-net/build.gradle.kts` 中：

```kotlin
val cargoNdk by tasks.registering(Exec::class) {
    workingDir = rootProject.file("..")
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64",
                "-o", "src/main/jniLibs", "build", "--release", "-p", "beam-ffi")
}
tasks.withType<com.android.build.gradle.tasks.MergeSourceSetFolders> {
    dependsOn(cargoNdk)
}
```

---

## 3. 生成物缺席时的行为

本工程**不依赖**生成物即可编译（所有交互走反射）。运行时缺生成物会抛出
**明确可读**的异常，而不是 `ClassNotFoundException` 或静默失败：

| 缺失物 | 异常 | 提示内容 |
|---|---|---|
| `libbeam_ffi.so` | `BeamNativeUnavailableException` | 「无法加载原生库 libbeam_ffi.so … 请用 cargo-ndk 构建并把产物复制到 jniLibs/arm64-v8a 与 x86_64」 |
| `BeamClient` 类 | `BeamNativeUnavailableException` | 「未找到 uniffi 生成的 BeamClient 类（尝试过：…）」 |
| `BeamClient.browse` 等方法 | `BeamNativeUnavailableException` | 「uniffi 生成的 BeamClient 缺少方法 browse …」 |

`NativeBeamBridge.isAvailable()` 供 UI 区分「so 没构建」与「真的没找到主机」。

---

## 4. uniffi 导出面约定

`NativeBeamBridge` 按下表反射调用 `beam-ffi` 的生成物。若 Rust 侧改名，
请同步这里与本表（否则会得到「缺少方法 X」的可读错误）。

| 生成物 | 期望形状 |
|---|---|
| `BeamClient(config: BeamClientConfig)` | `browse(listener)` / `pairQr(payload)` / `pairPin(peer, pin)` / `connect(peer, cfg, sink)` |
| `FrameSink` | `onFrame(data, ptsUs, captureTsUs, key)` / `onEvent(event)` /（可选）`onAudio(data, ptsUs)` |
| `DiscoveryListener` | `onHost(host)` |
| `Peer` | record：`id` `name` `address` `fingerprint` `platform` `trusted` |
| `StreamCfg` | record：`width` `height` `fps` `codec` `bitrateKbps` `fecRatio` `audioEnabled` |
| `BeamClientConfig` | record：`clientName` `logLevel` `fecRatio` `jitterFrames` `connectTimeoutMs` `reconnectTimeoutMs` |
| `SessionEvent` | 变体名：`Stats` `Reconnecting` `Disconnected` `StateChanged` `Error` |
| `InputEvent` | 变体名：`Key` `MouseMove` `MouseButton` `MouseWheel` `Touch` `GamepadButton` `GamepadAxis` |

record 的构造**按参数名**装配（`kotlin-reflect` 的 `callBy`），因此 Rust 字段
顺序变化不会破坏适配层。

---

## 5. 热路径：DirectByteBuffer 池化

§6 内存所有权约定：「JNI 热路径 DirectByteBuffer 由 Rust 侧池化，回调返回后归还」。

`NativeBeamBridge.deliverFrame` 的行为：

1. Rust 传入 `ByteBuffer` → 直接接管所有权；
2. Rust 传入 `ByteArray` → 从 `ByteBufferPool` 租一块直接内存并拷贝；
3. 消费方实现 `PooledFrameSink` → **所有权转移**给消费方，由其在消费完毕后
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
| `Session.state` | `StateChanged` 事件驱动 | §3.9 会话状态机 |

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
