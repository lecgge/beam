package dev.beam.core.net

import dev.beam.core.net.buffer.BufferLease
import dev.beam.core.net.buffer.ByteBufferPool
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import java.lang.reflect.InvocationHandler
import java.lang.reflect.Method
import java.lang.reflect.Proxy
import java.nio.ByteBuffer

/**
 * uniffi 生成绑定的反射适配层。
 *
 * ## 为什么是反射
 *
 * `:core-net` 必须能在**没有** uniffi 生成物的环境下编译（生成物由
 * `cargo-ndk` + `uniffi-bindgen` 在构建流水线里产出，属于构建产物而非源码）。
 * 因此这里不 import 任何生成类，一律通过反射调用。
 *
 * 生成物的形状约定（由 `beam-ffi` 的 UDL 决定，见 `core-net/README.md`）：
 *
 * | 生成物 | 期望形状 |
 * |---|---|
 * | `BeamClient` | `class BeamClient(config: BeamClientConfig)`，方法 `browse(listener)` / `pairQr(payload)` / `pairPin(peer, pin)` / `connect(peer, cfg, sink)` |
 * | `FrameSink` | `interface FrameSink { onFrame(data, ptsUs, captureTsUs, key); onEvent(event); onAudio?(data, ptsUs) }` |
 * | `DiscoveryListener` | `interface DiscoveryListener { onHost(host) }` |
 * | `Peer` / `StreamCfg` / `BeamClientConfig` | record，字段名与本文档注释一致 |
 *
 * ## 线程模型（§3.12）
 *
 * - Rust tokio 运行时在 `.so` 内；
 * - 帧回调发生在 Rust 派发的专用线程，**不在主线程**；
 * - 控制面事件（`SessionEvent`）经本类转成 Flow，由 UI 在主线程消费。
 *
 * ## 禁止事项
 *
 * 本类是 `:core-net` 内**唯一**允许触碰 JNI / `System.loadLibrary` 的地方。
 * `:feature-discovery` 等界面层必须经 [BeamClient] 访问 Rust 能力（§3.12）。
 */
class NativeBeamBridge private constructor(
    private val nativeClient: Any,
    private val pool: ByteBufferPool,
    private val scope: CoroutineScope,
) : BeamClient {

    // ------------------------------------------------------------------ 发现

    override fun discover(): Flow<DiscoveredHost> = callbackFlow {
        val listener = newDiscoveryListener { host -> trySend(host) }
        val browse = findMethod(nativeClient.javaClass, BROWSE_NAMES, 1)
            ?: throw BeamNativeUnavailableException(missingMessage("browse"))
        Reflect.invoke(nativeClient, browse, listener)
        awaitClose {
            // 停止浏览：生成物若提供 stopBrowsing 则调用，否则依赖 Rust 侧 drop
            findMethod(nativeClient.javaClass, STOP_BROWSE_NAMES, 1)?.let {
                runCatching { Reflect.invoke(nativeClient, it, listener) }
            }
        }
    }

    // ------------------------------------------------------------------ 配对

    override suspend fun pairQr(payload: String): PairResult {
        val method = findMethod(nativeClient.javaClass, PAIR_QR_NAMES, 1)
            ?: throw BeamNativeUnavailableException(missingMessage("pairQr"))
        return try {
            // uniffi 提升 Result 后：成功直接返回 `Peer` 记录，失败抛 `BeamException`。
            NativeMappings.mapPairResult(Reflect.invoke(nativeClient, method, payload))
        } catch (e: BeamNativeUnavailableException) {
            throw e
        } catch (e: Throwable) {
            NativeMappings.pairFailure(e)
        }
    }

    override suspend fun pairPin(peer: Peer, pin: String): PairResult {
        val peerClass = requireClass(PEER_CLASSES, "Peer")
        val method = findMethod(nativeClient.javaClass, PAIR_PIN_NAMES, 2)
            ?: throw BeamNativeUnavailableException(missingMessage("pairPin"))
        val peerRecord = Reflect.buildRecord(peerClass, peerFields(peer))
        return try {
            // uniffi 提升 Result 后：成功返回 `Unit`，失败抛 `BeamException`。
            NativeMappings.mapPairResult(Reflect.invoke(nativeClient, method, peerRecord, pin), peer)
        } catch (e: BeamNativeUnavailableException) {
            throw e
        } catch (e: Throwable) {
            NativeMappings.pairFailure(e)
        }
    }

    // ------------------------------------------------------------------ 会话

    override suspend fun connect(peer: Peer, cfg: StreamCfg, sink: FrameSink): Session =
        connect(peer, cfg) { sink }

    override suspend fun connect(peer: Peer, cfg: StreamCfg, sinkFactory: (StreamCfg) -> FrameSink): Session {
        val peerClass = requireClass(PEER_CLASSES, "Peer")
        val cfgClass = requireClass(CFG_CLASSES, "StreamCfg")
        val sinkClass = requireClass(SINK_CLASSES, "FrameSink")

        val peerRecord = Reflect.buildRecord(peerClass, peerFields(peer))
        val cfgRecord = Reflect.buildRecord(cfgClass, cfgFields(cfg))

        val bus = SessionBus()
        val sink = sinkFactory(cfg)
        val proxy = newFrameSinkProxy(sinkClass, sink, bus)

        val method = findMethod(nativeClient.javaClass, CONNECT_NAMES, 3)
            ?: throw BeamNativeUnavailableException(missingMessage("connect"))
        val handle = Reflect.invoke(nativeClient, method, peerRecord, cfgRecord, proxy)
            ?: throw BeamNativeException("uniffi connect 返回空句柄")

        return NativeSession(handle, bus, scope)
    }

    override fun close() {
        findMethod(nativeClient.javaClass, CLOSE_NAMES, 0)?.let {
            runCatching { Reflect.invoke(nativeClient, it) }
        }
        scope.cancel()
    }

    // ------------------------------------------------------------- 内部实现

    private fun newDiscoveryListener(onHost: (DiscoveredHost) -> Unit): Any {
        val iface = requireClass(LISTENER_CLASSES, "DiscoveryListener")
        return newProxy(iface, "DiscoveryListener") { proxy, method, args ->
            when (method.name) {
                "onHost", "on_host", "onDiscoveredHost", "onFound" -> {
                    val raw = args?.firstOrNull()
                    if (raw != null) onHost(mapDiscoveredHost(raw))
                }
                else -> defaultProxyResult(method, proxy, args)
            }
        }
    }

    private fun newFrameSinkProxy(iface: Class<*>, sink: FrameSink, bus: SessionBus): Any =
        newProxy(iface, "FrameSink") { proxy, method, args ->
            when (method.name) {
                "onFrame", "on_frame" -> {
                    val data = args?.getOrNull(0)
                    val ptsUs = Reflect.asLong(args?.getOrNull(1)) ?: 0L
                    val captureTsUs = Reflect.asLong(args?.getOrNull(2)) ?: 0L
                    val key = (args?.getOrNull(3) as? Boolean) ?: false
                    deliverFrame(sink, data, ptsUs, captureTsUs, key)
                }
                "onEvent", "on_event" -> {
                    val raw = args?.firstOrNull() ?: return@newProxy null
                    val mapped = NativeMappings.mapSessionEvent(raw, bus.negotiatedCfg)
                    if (mapped is SessionEvent.Negotiated) {
                        // 记住协商结果：Rust 的统计里没有分辨率与目标帧率。
                        bus.negotiatedCfg = mapped.cfg
                    }
                    mapped?.let { bus.events.tryEmit(it) }
                }
                "onAudio", "on_audio" -> {
                    val data = args?.getOrNull(0)
                    val ptsUs = Reflect.asLong(args?.getOrNull(1)) ?: 0L
                    val bytes = when (data) {
                        is ByteArray -> data
                        is ByteBuffer -> ByteArray(data.remaining()).also { data.get(it) }
                        else -> null
                    }
                    if (bytes != null) bus.audio.tryEmit(AudioPacket(bytes, ptsUs))
                }
                else -> defaultProxyResult(method, proxy, args)
            }
        }

    /**
     * 帧热路径：DirectByteBuffer 池化。
     *
     * 1. Rust 侧传入 [ByteBuffer] 时直接接管所有权；
     * 2. Rust 侧传入 [ByteArray] 时从 Kotlin 池里租一块直接内存并拷贝；
     * 3. 消费方实现 [PooledFrameSink] 时所有权转移给消费方，否则回调返回后立即归还。
     */
    private fun deliverFrame(
        sink: FrameSink,
        data: Any?,
        ptsUs: Long,
        captureTsUs: Long,
        key: Boolean,
    ) {
        val buffer: ByteBuffer
        val lease: BufferLease
        when (data) {
            is ByteBuffer -> {
                buffer = data
                lease = BufferLease(pool, data)
            }
            is ByteArray -> {
                val rented = pool.rentLease(maxOf(data.size, 1))
                rented.buffer.put(data)
                rented.buffer.flip()
                buffer = rented.buffer
                lease = rented
            }
            else -> return
        }

        if (sink is PooledFrameSink) {
            // 所有权转移：消费方负责 close() 归还。
            sink.onPooledFrame(PooledFrame(buffer, ptsUs, captureTsUs, key, lease))
        } else {
            try {
                sink.onFrame(buffer, ptsUs, captureTsUs, key)
            } finally {
                // 回调返回后归还（§6 内存所有权约定）。
                lease.close()
            }
        }
    }

    private fun mapDiscoveredHost(raw: Any): DiscoveredHost =
        NativeMappings.mapDiscoveredHost(raw, DiscoverySource.MDNS)

    private fun peerFields(peer: Peer): Map<String, Any?> = mapOf(
        "id" to peer.id,
        "name" to peer.name,
        "address" to peer.address,
        "fingerprint" to peer.fingerprint,
        "platform" to peer.platform,
        "trusted" to peer.trusted,
    )

    private fun cfgFields(cfg: StreamCfg): Map<String, Any?> = mapOf(
        "width" to cfg.width,
        "height" to cfg.height,
        "fps" to cfg.fps,
        "codec" to cfg.codec.name,
        "bitrateKbps" to cfg.bitrateKbps,
        "bitrate_kbps" to cfg.bitrateKbps,
        "fecRatio" to cfg.fecRatio,
        "fec_ratio" to cfg.fecRatio,
        "audioEnabled" to cfg.audioEnabled,
        "audio_enabled" to cfg.audioEnabled,
    )

    private fun requireClass(candidates: List<String>, label: String): Class<*> =
        Reflect.findClass(candidates)
            ?: throw BeamNativeUnavailableException(
                "未找到 uniffi 生成的 $label（尝试过：${candidates.joinToString()}）。" +
                    "请先执行 core-net/README.md 中的 uniffi 生成步骤，并把 libbeam_ffi.so 放进 jniLibs。",
            )

    private fun findMethod(cls: Class<*>, names: List<String>, argCount: Int): Method? {
        for (n in names) {
            Reflect.findMethod(cls, n, argCount)?.let { return it }
        }
        return null
    }

    private fun newProxy(
        iface: Class<*>,
        label: String,
        handler: (Any, Method, Array<out Any?>?) -> Any?,
    ): Any = try {
        Proxy.newProxyInstance(iface.classLoader, arrayOf(iface)) { proxy, method, args ->
            handler(proxy, method, args)
        }
    } catch (e: IllegalArgumentException) {
        throw BeamNativeUnavailableException("无法为 $label 创建动态代理（${iface.name}）", e)
    }

    private fun defaultProxyResult(method: Method, proxy: Any, args: Array<out Any?>?): Any? =
        when (method.name) {
            "toString" -> "BeamNativeProxy"
            "hashCode" -> System.identityHashCode(proxy)
            "equals" -> proxy === args?.getOrNull(0)
            else -> null
        }

    private fun missingMessage(member: String): String =
        "uniffi 生成的 BeamClient 缺少方法 $member。" +
            "请确认 crates/beam-ffi 的导出面与 core-net/README.md 的一致，并重新生成 Kotlin 绑定。"

    companion object {

        /** 加载的原生库名（`libbeam_ffi.so`）。 */
        const val LIBRARY_NAME: String = "beam_ffi"

        private val PACKAGES = listOf(
            "dev.beam.core.net.uniffi",
            "uniffi.beam_ffi",
            "dev.beam.ffi",
        )

        private fun candidates(name: String): List<String> = PACKAGES.map { "$it.$name" }

        val CLIENT_CLASSES: List<String> = candidates("BeamClient")
        val SINK_CLASSES: List<String> = candidates("FrameSink")
        val LISTENER_CLASSES: List<String> = candidates("DiscoveryListener")
        val PEER_CLASSES: List<String> = candidates("Peer")
        val CFG_CLASSES: List<String> = candidates("StreamCfg")
        val CONFIG_CLASSES: List<String> = candidates("BeamClientConfig")
        val INPUT_EVENT_CLASSES: List<String> = candidates("InputEvent")

        private val BROWSE_NAMES = listOf("browse", "startBrowsing")
        private val STOP_BROWSE_NAMES = listOf("stopBrowsing", "unbrowse")
        private val PAIR_QR_NAMES = listOf("pairQr", "pair_qr")
        private val PAIR_PIN_NAMES = listOf("pairPin", "pair_pin")
        private val CONNECT_NAMES = listOf("connect")
        private val CLOSE_NAMES = listOf("close", "shutdown", "dispose")

        /**
         * 加载原生库并构造反射桥。
         *
         * @throws BeamNativeUnavailableException 当 `.so` 缺失或 uniffi 生成类不存在时。
         */
        fun create(
            config: BeamClientConfig = BeamClientConfig(),
            pool: ByteBufferPool = ByteBufferPool(),
            scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default),
        ): NativeBeamBridge {
            loadNativeLibrary()
            val clientClass = Reflect.findClass(CLIENT_CLASSES)
                ?: throw BeamNativeUnavailableException(
                    "未找到 uniffi 生成的 BeamClient 类（尝试过：${CLIENT_CLASSES.joinToString()}）。" +
                        "请执行 core-net/README.md 中的 uniffi-bindgen 生成步骤。",
                )
            val configClass = Reflect.findClass(CONFIG_CLASSES)
            val configRecord = if (configClass != null) {
                Reflect.buildRecord(
                    configClass,
                    mapOf(
                        "clientName" to config.clientName,
                        "client_name" to config.clientName,
                        "logLevel" to config.logLevel,
                        "log_level" to config.logLevel,
                        "fecRatio" to config.fecRatio,
                        "fec_ratio" to config.fecRatio,
                        "jitterFrames" to config.jitterFrames,
                        "jitter_frames" to config.jitterFrames,
                        "connectTimeoutMs" to config.connectTimeoutMs,
                        "connect_timeout_ms" to config.connectTimeoutMs,
                        "reconnectTimeoutMs" to config.reconnectTimeoutMs,
                        "reconnect_timeout_ms" to config.reconnectTimeoutMs,
                    ),
                )
            } else {
                null
            }
            val ctor = clientClass.constructors.firstOrNull { it.parameterCount == 1 }
                ?: throw BeamNativeUnavailableException(
                    "uniffi 生成的 BeamClient 没有单参数构造函数，无法传入 BeamClientConfig。",
                )
            val nativeClient = try {
                ctor.newInstance(configRecord)
            } catch (e: java.lang.reflect.InvocationTargetException) {
                throw BeamNativeException("构造 uniffi BeamClient 失败", e.targetException)
            }
            return NativeBeamBridge(nativeClient, pool, scope)
        }

        /** 加载 `libbeam_ffi.so`；缺失时抛出带修复指引的异常。 */
        fun loadNativeLibrary() {
            try {
                System.loadLibrary(LIBRARY_NAME)
            } catch (e: UnsatisfiedLinkError) {
                throw BeamNativeUnavailableException(
                    "无法加载原生库 lib$LIBRARY_NAME.so：${e.message}。" +
                        "请用 cargo-ndk 构建并把产物复制到 core-net/src/main/jniLibs/arm64-v8a/ 与 jniLibs/x86_64/。" +
                        "（详见 core-net/README.md）",
                    e,
                )
            }
        }

        /** 生成物是否已经就位；用于 UI 上区分「未构建 so」与「真的没有主机」。 */
        fun isAvailable(): Boolean = runCatching {
            loadNativeLibrary()
            Reflect.findClass(CLIENT_CLASSES) != null
        }.getOrDefault(false)
    }
}

/**
 * Rust 侧客户端配置。
 *
 * 与 `beam-ffi` 的 `BeamClientConfig` 字段一一对应（按名字反射装配）。
 */
data class BeamClientConfig(
    /** 客户端标识，出现在主机端「已配对设备」列表里。 */
    val clientName: String = "beam-android",
    /** 日志级别：`error` / `warn` / `info` / `debug` / `trace`。 */
    val logLevel: String = "info",
    /** FEC 冗余比例（§3.2）。 */
    val fecRatio: Float = 0.2f,
    /** 抖动缓冲帧数（§3.2 有线 0，Wi-Fi 自适应 0–1）。 */
    val jitterFrames: Int = 0,
    /** 建连超时（毫秒）。 */
    val connectTimeoutMs: Int = 10_000,
    /** 重连窗口（毫秒，§3.9 `reconnect_timeout=30s`）。 */
    val reconnectTimeoutMs: Int = 30_000,
)

/**
 * 会话控制面总线：把 Rust 回调转成 Flow（§3.12「回调转 Flow」）。
 */
internal class SessionBus {
    val events = MutableSharedFlow<SessionEvent>(
        replay = 0,
        extraBufferCapacity = 256,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    val audio = MutableSharedFlow<AudioPacket>(
        replay = 0,
        extraBufferCapacity = 64,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )

    /**
     * 最近一次能力协商的结果。
     *
     * Rust 的 `SessionStats` 不带分辨率与目标帧率，这两个字段只能由协商结果补齐。
     * 回调线程写、统计映射线程读，用 `@Volatile` 保证可见性。
     */
    @Volatile
    var negotiatedCfg: StreamCfg? = null
}

/**
 * 基于反射句柄的会话实现。
 */
private class NativeSession(
    private val handle: Any,
    private val bus: SessionBus,
    scope: CoroutineScope,
) : Session {

    private val _state = kotlinx.coroutines.flow.MutableStateFlow(SessionState.IDLE)
    override val state: kotlinx.coroutines.flow.StateFlow<SessionState> = _state

    override val events: Flow<SessionEvent> = bus.events.asSharedFlow()

    override val stats: Flow<SessionStats> = bus.events
        .filterIsInstance<SessionEvent.Stats>()
        .map { it.stats }

    override val audioPackets: Flow<AudioPacket> = bus.audio.asSharedFlow()

    init {
        // 初始状态直接问一次原生句柄（`Session::state()`），
        // 避免「已连接但还没收到 StateChanged 事件」这段时间里状态显示为 IDLE。
        Reflect.findMethodByName(handle.javaClass, "state")?.let { method ->
            runCatching { Reflect.invoke(handle, method) }
                .getOrNull()
                ?.let { raw ->
                    Reflect.readEnumName(raw, "name")
                        ?: Reflect.readString(raw, "name")
                        ?: raw.toString()
                }
                ?.let { name -> _state.value = NativeMappings.parseSessionState(name) }
        }
        // 状态机迁移由事件驱动（§3.9 / §4.1）。
        scope.launch {
            bus.events.collect { event ->
                if (event is SessionEvent.StateChanged) {
                    _state.value = event.state
                }
            }
        }
    }

    override fun sendInput(event: InputEvent) {
        // Rust 侧导出面目前**没有**输入通道（`crates/beam-ffi/src/ffi.rs` 的
        // `Session` 只导出 `state()` / `stop()`）。这里如实报错，不做假实现。
        val method = Reflect.findMethodByName(handle.javaClass, "sendInput")
            ?: Reflect.findMethodByName(handle.javaClass, "send_input")
            ?: throw BeamNativeUnavailableException(
                "uniffi 会话句柄缺少 sendInput 方法。" +
                    "Rust 侧需要在 crates/beam-ffi/src/ffi.rs 的 Session 导出面补 " +
                    "`pub fn send_input(&self, event: InputEvent)`，" +
                    "并导出与 beam_proto::InputEvent 对齐的 InputEvent 枚举。",
            )
        val inputClass = Reflect.findClass(NativeBeamBridge.INPUT_EVENT_CLASSES)
            ?: throw BeamNativeUnavailableException(
                "未找到 uniffi 生成的 InputEvent 类（尝试过：${NativeBeamBridge.INPUT_EVENT_CLASSES.joinToString()}）。" +
                    "Rust 侧需要用 #[derive(uniffi::Enum)] 导出 InputEvent。",
            )
        Reflect.invoke(handle, method, buildInputEvent(inputClass, event))
    }

    override fun requestIdr() {
        val method = Reflect.findMethodByName(handle.javaClass, "requestIdr")
            ?: Reflect.findMethodByName(handle.javaClass, "request_idr")
            ?: throw BeamNativeUnavailableException(
                "uniffi 会话句柄缺少 requestIdr 方法。" +
                    "Rust 侧需要在 crates/beam-ffi/src/ffi.rs 的 Session 导出面补 " +
                    "`pub fn request_idr(&self)`。",
            )
        Reflect.invoke(handle, method)
    }

    override fun update(cfg: StreamCfg) {
        val cfgClass = Reflect.findClass(NativeBeamBridge.CFG_CLASSES)
            ?: throw BeamNativeUnavailableException(
                "未找到 uniffi 生成的 StreamCfg 类（尝试过：${NativeBeamBridge.CFG_CLASSES.joinToString()}）。",
            )
        val method = Reflect.findMethodByName(handle.javaClass, "update")
            ?: throw BeamNativeUnavailableException(
                "uniffi 会话句柄缺少 update 方法。" +
                    "Rust 侧需要在 crates/beam-ffi/src/ffi.rs 的 Session 导出面补 " +
                    "`pub fn update(&self, cfg: StreamCfg)`。",
            )
        Reflect.invoke(
            handle,
            method,
            Reflect.buildRecord(
                cfgClass,
                mapOf(
                    "width" to cfg.width,
                    "height" to cfg.height,
                    "fps" to cfg.fps,
                    "codec" to cfg.codec.name,
                    "bitrateKbps" to cfg.bitrateKbps,
                    "bitrate_kbps" to cfg.bitrateKbps,
                    "fecRatio" to cfg.fecRatio,
                    "fec_ratio" to cfg.fecRatio,
                    "audioEnabled" to cfg.audioEnabled,
                    "audio_enabled" to cfg.audioEnabled,
                ),
            ),
        )
    }

    override fun close() {
        listOf("close", "stop", "dispose").forEach { name ->
            Reflect.findMethodByName(handle.javaClass, name)?.let {
                runCatching { Reflect.invoke(handle, it) }
                return
            }
        }
    }

    /**
     * 把 Kotlin 侧的 [InputEvent] 装配成 uniffi 生成的输入事件对象。
     *
     * ## 形状容忍
     *
     * Rust 侧协议模型（`beam_proto::InputEvent`）与 Kotlin 侧模型的字段并不完全一致：
     * - 变体名可能不同（Kotlin 的 `MouseWheel` 对应 Rust 的 `Scroll`）；
     * - 按键动作的表达不同（Kotlin 的 `pressed: Boolean` 对应 Rust 的 `action: KeyAction`）；
     * - 字段命名风格不同（uniffi Kotlin 生成 camelCase，Rust 侧是 snake_case）。
     *
     * 因此这里对**变体名**给候选列表、对**字段名**同时提供 camelCase 与 snake_case，
     * 由 [Reflect.buildRecord] 按参数名挑真正存在的那一个。
     */
    private fun buildInputEvent(cls: Class<*>, event: InputEvent): Any {
        val kClass = cls.kotlin
        val variantCandidates = when (event) {
            is InputEvent.Key -> listOf("Key")
            is InputEvent.MouseMove -> listOf("MouseMove")
            is InputEvent.MouseButton -> listOf("MouseButton")
            is InputEvent.MouseWheel -> listOf("MouseWheel", "Scroll")
            is InputEvent.Touch -> listOf("Touch")
            is InputEvent.GamepadButton -> listOf("GamepadButton")
            is InputEvent.GamepadAxis -> listOf("GamepadAxis")
        }
        val variantClass = variantCandidates
            .asSequence()
            .mapNotNull { name -> kClass.sealedSubclasses.firstOrNull { it.simpleName == name } }
            .firstOrNull()
            ?.java
            ?: throw BeamNativeException(
                "uniffi InputEvent 缺少变体 ${variantCandidates.joinToString(" / ")}",
            )

        val values = mutableMapOf<String, Any?>()
        values["timestampUs"] = event.timestampUs
        values["timestamp_us"] = event.timestampUs
        when (event) {
            is InputEvent.Key -> {
                values["code"] = event.code
                values["pressed"] = event.pressed
                values["repeat"] = event.repeat
                values["action"] = if (event.pressed) "Down" else "Up"
            }
            is InputEvent.MouseMove -> {
                values["dx"] = event.dx
                values["dy"] = event.dy
                values["x"] = event.dx
                values["y"] = event.dy
            }
            is InputEvent.MouseButton -> {
                values["button"] = event.button
                values["pressed"] = event.pressed
                values["action"] = if (event.pressed) "Press" else "Release"
            }
            is InputEvent.MouseWheel -> {
                values["deltaX"] = event.deltaX
                values["deltaY"] = event.deltaY
                values["delta_x"] = event.deltaX
                values["delta_y"] = event.deltaY
                values["dx"] = event.deltaX
                values["dy"] = event.deltaY
            }
            is InputEvent.Touch -> {
                values["pointerId"] = event.pointerId
                values["pointer_id"] = event.pointerId
                values["action"] = event.action.name
                values["x"] = event.x
                values["y"] = event.y
                values["normalizedX"] = event.normalizedX
                values["normalizedY"] = event.normalizedY
                values["normalized_x"] = event.normalizedX
                values["normalized_y"] = event.normalizedY
                values["pressure"] = event.pressure
            }
            is InputEvent.GamepadButton -> {
                values["button"] = event.button.name
                values["pressed"] = event.pressed
                values["action"] = if (event.pressed) "Press" else "Release"
            }
            is InputEvent.GamepadAxis -> {
                values["axis"] = event.axis.name
                values["value"] = event.value
            }
        }
        return Reflect.buildRecord(variantClass, values)
    }
}
