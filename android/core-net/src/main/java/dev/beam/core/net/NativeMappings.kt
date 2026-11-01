package dev.beam.core.net

/**
 * uniffi 生成物 → `:core-net` 模型的映射层。
 *
 * ## 为什么单独成文件
 *
 * [NativeBeamBridge] 必须持有真实生成的 Kotlin 类（`uniffi.beam_ffi.*`）才能工作，
 * 在没有生成物的环境里**根本无法实例化**，因此写在其内部的映射逻辑也无法被单测覆盖。
 * 本文件把映射逻辑抽成**只依赖反射**的纯函数：输入是「任意形状的对象」，
 * 输出是 `:core-net` 的模型。这样：
 *
 * - 单测可以用与生成物同构的替身类覆盖（见 `NativeMappingsTest`）；
 * - 生成物形状变化时，改动集中在本文件，而不是散落在反射胶水层。
 *
 * ## 生成物的实际形状（来源：`crates/beam-ffi/src/ffi.rs` 的 uniffi 导出面）
 *
 * | 生成物 | 实际形状 |
 * |---|---|
 * | `BeamClient` | `class BeamClient(config: BeamClientConfig)`，方法 `browse` / `pairQr` / `pairPin` / `connect` |
 * | `Session` | `class Session`，方法 `state()` / `stop()` / `reconnecting()` / `summary()` |
 * | `SessionEvent` | `sealed class`，变体 `StateChanged` / `Paired` / `Negotiated` / `Stats` / `Error` / `Closed` |
 * | `SessionEvent.Stats` | **嵌套**记录：统计字段在 `stats: SessionStats` 里，不在变体上 |
 * | `SessionStats` | `sessionId: ULong` / `fps: UInt` / `bitrateBps: UInt` / `lossPpm: UInt` / `rttUs: UInt` / `decodeUs: UInt` / `renderUs: UInt` / `e2eUs: UInt` / `droppedFrames: UInt` |
 * | `ReconnectingInfo` | `elapsedMs: ULong` / `keepLastFrame: Boolean` —— 由 `Session::reconnecting()` **查询**得到（不在重连态时返回 `null`） |
 * | `SessionSummary` | `sessionId: ULong` / `durationMs: ULong` / `frames: ULong` / `droppedFrames: ULong` / `reconnects: UInt` / `rttUs: UInt` / `lossPpm: UInt` / `fps: UInt` / `bitrateBps: UInt` / `state: SessionState` |
 * | `DiscoveredHost` | `id` / `name` / `version` / `fingerprint` / `port: UInt` / `address` / `fullname` —— **地址与端口是分开的两个字段** |
 * | `Peer` | `id` / `name` / `address` / `fingerprint` / `platform` / `trusted` |
 * | `StreamCfg` | `width` / `height` / `fps` / `codec: String` / `bitrateKbps` / `fecRatio` / `audioEnabled` |
 * | `BeamClientConfig` | `clientName` / `logLevel` / `fecRatio` / `jitterFrames` / `connectTimeoutMs` / `reconnectTimeoutMs` |
 *
 * ## 单位约定
 *
 * Rust 侧统计一律用**整数最小单位**（µs / ppm / bit·s⁻¹），
 * Kotlin 侧模型用**UI 友好单位**（ms / 百分比 / kbit·s⁻¹），换算集中在本文件。
 */
internal object NativeMappings {

    /**
     * uniffi 提升 `Result<T, E>` 后，失败时抛出的异常变体 → FFI 层错误码。
     *
     * Rust 侧 `BeamError` 的 `#[error("E1400x: ...")]` 文案**不会**进入 uniffi 的
     * 异常消息（生成物里 `PinFailed.message` 是空串），因此只能按**异常类型名**映射。
     */
    private val FFI_ERROR_CODES: Map<String, String> = mapOf(
        "InvalidPayload" to "E14001",
        "PinFailed" to "E14002",
        "ConnectFailed" to "E14003",
        "DiscoveryFailed" to "E14004",
        "Unsupported" to "E14005",
    )

    /** 未知 FFI 层错误码。 */
    private const val UNKNOWN_FFI_CODE: String = "E14000"

    /**
     * 把 uniffi 的 `SessionEvent` 映射成 `:core-net` 的 [SessionEvent]。
     *
     * @param negotiated 最近一次协商出的串流配置；Rust 的统计里**没有**分辨率与目标帧率，
     *   这两个字段只能由协商结果补齐（见 [mapStats]）。
     */
    fun mapSessionEvent(raw: Any, negotiated: StreamCfg? = null): SessionEvent? =
        when (raw.javaClass.simpleName) {
            "Stats" -> SessionEvent.Stats(mapStats(raw, negotiated))

            "StateChanged" -> {
                val next = Reflect.readEnumName(raw, "state") ?: return null
                val previous = Reflect.readEnumName(raw, "previous") ?: next
                SessionEvent.StateChanged(
                    state = parseSessionState(next),
                    previous = parseSessionState(previous),
                )
            }

            "Error" -> SessionEvent.Error(
                code = Reflect.readString(raw, "code") ?: "E9000",
                message = Reflect.readString(raw, "message") ?: "会话错误",
                retryable = Reflect.readBoolean(raw, "retryable") ?: false,
            )

            // 能力协商完成：Rust 侧带回最终的串流配置。
            "Negotiated" -> {
                val cfg = mapStreamCfg(Reflect.read(raw, "config")) ?: return null
                SessionEvent.Negotiated(cfg)
            }

            // 会话结束：Rust 侧的终态变体，对应 Android 侧的「连接已断开」。
            "Closed" -> SessionEvent.Disconnected(
                code = "E9000",
                message = "会话已结束",
                retryable = false,
            )

            // 丢帧（§3.2 E2004）：**不是**会话错误，只作统计 / 提示。
            "FramesDropped" -> SessionEvent.FramesDropped(
                count = Reflect.readInt(raw, "count") ?: 0,
                code = Reflect.readInt(raw, "code")?.let { "E$it" } ?: "E2004",
            )

            // 以下两个变体当前 Rust 导出面没有，但保留映射以兼容后续扩展。
            "Reconnecting" -> SessionEvent.Reconnecting(
                attempt = Reflect.readInt(raw, "attempt") ?: 0,
                reason = Reflect.readString(raw, "reason"),
                elapsedMs = Reflect.readLong(raw, "elapsedMs", "elapsed_ms") ?: 0L,
            )

            "Disconnected" -> SessionEvent.Disconnected(
                code = Reflect.readString(raw, "code") ?: "E9000",
                message = Reflect.readString(raw, "message") ?: "连接已断开",
                retryable = Reflect.readBoolean(raw, "retryable") ?: false,
            )

            // "Paired"：配对事件不进入串流会话事件流，忽略。
            else -> null
        }

    /**
     * 把 uniffi 的统计快照映射成 [SessionStats]。
     *
     * 关键点有两个：
     * 1. Rust 的 `SessionEvent::Stats { stats }` 是**嵌套**记录，统计字段在 `stats` 上；
     * 2. Rust 用整数最小单位（µs / ppm / bps），Kotlin 侧模型用 ms / 百分比 / kbps。
     *
     * @param negotiated 协商出的串流配置，用来补齐 Rust 侧没有的分辨率与目标帧率。
     */
    fun mapStats(raw: Any, negotiated: StreamCfg? = null): SessionStats {
        // 变体上的统计字段是嵌套记录；找不到时退回平铺形状（容忍未来导出面变化）。
        val source = Reflect.read(raw, "stats") ?: raw
        return SessionStats(
            fps = Reflect.readDouble(source, "fps") ?: 0.0,
            bitrateKbps = (Reflect.readDouble(source, "bitrateBps", "bitrate_bps") ?: 0.0) / 1000.0,
            rttMs = (Reflect.readDouble(source, "rttUs", "rtt_us") ?: 0.0) / 1000.0,
            lossPercent = (Reflect.readDouble(source, "lossPpm", "loss_ppm") ?: 0.0) / 10_000.0,
            fecRecoveredPercent = Reflect.readDouble(
                source,
                "fecRecoveredPercent",
                "fec_recovered_percent",
            ) ?: 0.0,
            decodeMs = (Reflect.readDouble(source, "decodeUs", "decode_us") ?: 0.0) / 1000.0,
            renderMs = (Reflect.readDouble(source, "renderUs", "render_us") ?: 0.0) / 1000.0,
            e2eMs = (Reflect.readDouble(source, "e2eUs", "e2e_us") ?: 0.0) / 1000.0,
            droppedFrames = Reflect.readLong(source, "droppedFrames", "dropped_frames") ?: 0L,
            width = negotiated?.width ?: 0,
            height = negotiated?.height ?: 0,
            fpsTarget = negotiated?.fps ?: 0,
            codec = negotiated?.codec ?: VideoCodec.HEVC,
        )
    }

    /** 把 uniffi 的 `StreamCfg` 记录映射成 [StreamCfg]；形状不合法时返回 null。 */
    fun mapStreamCfg(raw: Any?): StreamCfg? {
        if (raw == null) return null
        val width = Reflect.readInt(raw, "width") ?: 0
        val height = Reflect.readInt(raw, "height") ?: 0
        val fps = Reflect.readInt(raw, "fps") ?: 0
        if (width <= 0 || height <= 0 || fps <= 0) return null
        val fecRatio = (Reflect.readDouble(raw, "fecRatio", "fec_ratio") ?: 0.2).toFloat()
        return StreamCfg(
            width = width,
            height = height,
            fps = fps,
            codec = parseVideoCodec(Reflect.readString(raw, "codec")),
            bitrateKbps = Reflect.readInt(raw, "bitrateKbps", "bitrate_kbps") ?: 20_000,
            fecRatio = fecRatio.coerceIn(0f, 1f),
            audioEnabled = Reflect.readBoolean(raw, "audioEnabled", "audio_enabled") ?: false,
        )
    }

    /**
     * 把 uniffi 的 `ReconnectingInfo` 记录映射成 [ReconnectingInfo]。
     *
     * 输入 `null`（不在重连态）时返回 `null` —— 上层据此判断「有没有在重连」，
     * 而不是拿一个全零的记录去猜。
     *
     * 对应 Rust 侧 `beam_session::ReconnectingInfo`，
     * 经 uniffi 的 `Session::reconnecting()` 查询得到（§4.1）。
     */
    fun mapReconnectingInfo(raw: Any?): ReconnectingInfo? {
        if (raw == null) return null
        return ReconnectingInfo(
            elapsedMs = Reflect.readLong(raw, "elapsedMs", "elapsed_ms") ?: 0L,
            // 会话层在重连期间从不要求清屏 ⇒ 缺字段时按 true 处理。
            keepLastFrame = Reflect.readBoolean(raw, "keepLastFrame", "keep_last_frame") ?: true,
        )
    }

    /**
     * 把 uniffi 的 `SessionSummary` 记录映射成 [SessionSummary]。
     *
     * 对应 Rust 侧 `beam_session::SessionSummary`，
     * 经 uniffi 的 `Session::summary()` 查询得到（§3.9）。
     *
     * 单位换算与 [mapStats] 一致：µs → ms、ppm → 百分比、bit/s → kbit/s。
     */
    fun mapSessionSummary(raw: Any?): SessionSummary? {
        if (raw == null) return null
        val stateName = Reflect.readEnumName(raw, "state")
            ?: Reflect.readString(raw, "state")
        return SessionSummary(
            sessionId = Reflect.readLong(raw, "sessionId", "session_id") ?: 0L,
            durationMs = Reflect.readLong(raw, "durationMs", "duration_ms") ?: 0L,
            frames = Reflect.readLong(raw, "frames") ?: 0L,
            droppedFrames = Reflect.readLong(raw, "droppedFrames", "dropped_frames") ?: 0L,
            reconnects = Reflect.readInt(raw, "reconnects") ?: 0,
            rttMs = (Reflect.readDouble(raw, "rttUs", "rtt_us") ?: 0.0) / 1000.0,
            lossPercent = (Reflect.readDouble(raw, "lossPpm", "loss_ppm") ?: 0.0) / 10_000.0,
            fps = Reflect.readDouble(raw, "fps") ?: 0.0,
            bitrateKbps = (Reflect.readDouble(raw, "bitrateBps", "bitrate_bps") ?: 0.0) / 1000.0,
            state = if (stateName != null) parseSessionState(stateName) else SessionState.IDLE,
        )
    }

    /**
     * 把 uniffi 的 `Peer` 记录映射成 [Peer]。
     *
     * 平台字段：Rust 的 `PeerInfo` 目前没有平台字段（§3.3 的二维码载荷也不带），
     * 所以这里读到空串时如实记「未知平台」（S1 裁决），不猜成 windows。
     *
     * PIN 配对走的是 [mapPairResult] 的「保留请求方 Peer」分支，
     * 平台来自**发现**阶段拿到的 `plat`，因此 PIN 路径的平台是真实的。
     */
    fun mapPeer(raw: Any): Peer = Peer(
        id = Reflect.readString(raw, "id") ?: "",
        name = Reflect.readString(raw, "name") ?: "",
        address = Reflect.readString(raw, "address", "addr") ?: "",
        fingerprint = Reflect.readString(raw, "fingerprint", "fp") ?: "",
        platform = Reflect.readString(raw, "platform")?.takeIf { it.isNotEmpty() }
            ?: Peer.PLATFORM_UNKNOWN,
        trusted = Reflect.readBoolean(raw, "trusted") ?: false,
    )

    /**
     * 把 uniffi 的 `DiscoveredHost` 记录映射成 [DiscoveredHost]。
     *
     * Rust 侧把**地址与端口分成两个字段**，而 Kotlin 侧的 [Peer.address] 是
     * `host:port`（Rust 的 `split_address` 也按 `rsplit_once(':')` 解析），
     * 因此这里必须把端口拼回去，否则连不上主机。
     *
     * 平台来自 §3.4 mDNS TXT 的 `plat` 字段（S1 裁决）：对端没广播时
     * 如实记「未知平台」，**不猜**成 windows。
     */
    fun mapDiscoveredHost(
        raw: Any,
        source: DiscoverySource = DiscoverySource.MDNS,
    ): DiscoveredHost {
        val address = Reflect.readString(raw, "address", "addr") ?: ""
        val port = Reflect.readInt(raw, "port") ?: 0
        val peer = Peer(
            id = Reflect.readString(raw, "id") ?: "",
            name = Reflect.readString(raw, "name") ?: "",
            address = joinAddress(address, port),
            fingerprint = Reflect.readString(raw, "fingerprint", "fp") ?: "",
            platform = Reflect.readString(raw, "platform", "plat")?.takeIf { it.isNotEmpty() }
                ?: Peer.PLATFORM_UNKNOWN,
        )
        val lastSeen = Reflect.readLong(raw, "lastSeenMs", "last_seen_ms") ?: 0L
        return DiscoveredHost(peer, source, lastSeen)
    }

    /**
     * 把 uniffi 的配对返回值映射成 [PairResult]。
     *
     * uniffi 提升 `Result<T, E>` 后：
     * - `pair_qr` 成功时**直接返回 `Peer` 记录**（不是 `Ok(Peer)` 包装）；
     * - `pair_pin` 成功时返回 `Unit`；
     * - 失败时抛异常（由 [pairFailure] 处理）。
     *
     * @param requested 调用方传入的 `Peer`；`pair_pin` 成功时用它作为结果。
     */
    fun mapPairResult(raw: Any?, requested: Peer? = null): PairResult {
        if (raw == null) {
            return PairResult.Failure(
                code = "E14001",
                message = "配对失败：uniffi 返回空结果",
                retryable = true,
            )
        }
        if (raw is Unit) {
            // `pair_pin` 成功：uniffi 返回 Unit，配对结果就是请求里的那个 Peer。
            return requested?.let { PairResult.Success(it) }
                ?: PairResult.Failure(
                    code = "E14001",
                    message = "配对失败：uniffi 返回空结果",
                    retryable = true,
                )
        }
        val code = Reflect.readString(raw, "code")
        if (code != null) {
            // 兼容「错误被包成记录」的形状（UDL 模式下的常见形态）。
            return PairResult.Failure(
                code = code,
                message = Reflect.readString(raw, "message") ?: "配对失败",
                retryable = Reflect.readBoolean(raw, "retryable") ?: false,
                attemptsLeft = Reflect.readInt(raw, "attemptsLeft", "attempts_left"),
                lockoutMs = Reflect.readLong(raw, "lockoutMs", "lockout_ms"),
            )
        }
        // 成功：`pair_qr` 直接返回 Peer 记录。
        val peer = mapPeer(raw)
        return if (peer.address.isNotEmpty() || peer.fingerprint.isNotEmpty()) {
            PairResult.Success(peer)
        } else {
            PairResult.Failure(
                code = "E14001",
                message = "配对失败：uniffi 返回了无法识别的结果",
                retryable = true,
            )
        }
    }

    /**
     * 把 uniffi 抛出的异常映射成 [PairResult.Failure]。
     *
     * 异常类型名 → FFI 层错误码（见 [FFI_ERROR_CODES]）。
     */
    fun pairFailure(cause: Throwable): PairResult.Failure {
        val chain = generateSequence(cause) { it.cause }.toList()
        val uniffiError = chain.firstOrNull { FFI_ERROR_CODES.containsKey(it.javaClass.simpleName) }
        val code = uniffiError?.let { FFI_ERROR_CODES.getValue(it.javaClass.simpleName) }
            ?: UNKNOWN_FFI_CODE
        val reason = chain.firstNotNullOfOrNull { it.message?.takeIf { m -> m.isNotBlank() && m != code } }
        return PairResult.Failure(
            code = code,
            message = reason ?: "配对失败（$code）",
            retryable = false,
        )
    }

    /** 解析编解码器名（大小写不敏感，与 Rust 侧 `parse_codec` 一致）。 */
    fun parseVideoCodec(raw: String?): VideoCodec = when (raw?.trim()?.lowercase()) {
        "h264", "avc" -> VideoCodec.H264
        "av1" -> VideoCodec.AV1
        else -> VideoCodec.HEVC
    }

    /** 解析会话状态名（Rust 的 `Idle` / `Streaming` … 对应 Kotlin 的 `IDLE` / `STREAMING` …）。 */
    fun parseSessionState(name: String): SessionState = runCatching {
        SessionState.valueOf(name.trim().uppercase())
    }.getOrDefault(SessionState.IDLE)

    /**
     * 把「地址 + 端口」拼成 Rust 侧 `split_address` 能解析的 `host:port`。
     *
     * 地址里已经带端口时原样返回；端口缺失时只返回地址。
     */
    private fun joinAddress(address: String, port: Int): String {
        val host = address.trim()
        if (host.isEmpty()) return ""
        if (port <= 0) return host
        if (host.contains(':')) return host
        return "$host:$port"
    }
}
