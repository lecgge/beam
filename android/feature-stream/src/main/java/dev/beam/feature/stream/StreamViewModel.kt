package dev.beam.feature.stream

import android.view.Surface
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import dev.beam.core.audio.AudioPlayer
import dev.beam.core.audio.LowLatencyAudioPlayer
import dev.beam.core.data.SettingsRepo
import dev.beam.core.decoder.DecoderCodec
import dev.beam.core.decoder.MediaCodecVideoDecoder
import dev.beam.core.decoder.VideoDecoder
import dev.beam.core.decoder.VideoFormat
import dev.beam.core.net.BeamClient
import dev.beam.core.net.FrameSink
import dev.beam.core.net.InputEvent
import dev.beam.core.net.Peer
import dev.beam.core.net.Session
import dev.beam.core.net.SessionEvent
import dev.beam.core.net.SessionStats
import dev.beam.core.net.StreamCfg
import dev.beam.core.renderer.FramePacer
import dev.beam.core.renderer.PacerDecision
import dev.beam.core.renderer.SurfaceFrameRate
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.nio.ByteBuffer

/**
 * 串流阶段。
 */
enum class StreamPhase {
    /** 未连接。 */
    IDLE,

    /** 连接中。 */
    CONNECTING,

    /** 串流中。 */
    STREAMING,

    /** 断线重连中（§4.1：3 s 无数据 → Reconnecting，30 s 内恢复）。 */
    RECONNECTING,

    /** 已断开。 */
    DISCONNECTED,

    /** 出错。 */
    ERROR,
}

/**
 * 串流页状态。
 */
data class StreamUiState(
    val phase: StreamPhase = StreamPhase.IDLE,
    val peer: Peer? = null,
    /** `session://stats` 的最新快照（500 ms 一次）。 */
    val stats: SessionStats? = null,
    /** 提示文案。 */
    val message: String? = null,
    /** 统计浮层是否可见。 */
    val overlayVisible: Boolean = true,
    /** 悬浮工具栏是否展开。 */
    val toolbarVisible: Boolean = true,
    val resolutionLabel: String = "",
    val codecLabel: String = "",
    /** 客户端丢弃的帧数（本地统计）。 */
    val droppedFrames: Long = 0L,
)

/**
 * 串流 ViewModel（§3.12 `:feature-stream` 的对外接口）。
 *
 * ## 职责边界
 * - 负责：会话生命周期、Surface 与解码器的对接、输入上行、统计浮层状态；
 * - **不负责**：解码细节（在 `:core-decoder`）、上屏节拍（在 `:core-renderer`）。
 */
class StreamViewModel(
    private val client: BeamClient,
    private val settingsRepo: SettingsRepo,
    private val decoderFactory: () -> VideoDecoder = { MediaCodecVideoDecoder() },
    private val audioPlayerFactory: () -> AudioPlayer = { LowLatencyAudioPlayer() },
) : ViewModel() {

    private val _state = MutableStateFlow(StreamUiState())
    val state: StateFlow<StreamUiState> = _state.asStateFlow()

    private var session: Session? = null
    private var decoder: VideoDecoder? = null
    private var audioPlayer: AudioPlayer? = null
    private var pacer: FramePacer? = null
    private var surface: Surface? = null
    private var streamCfg: StreamCfg? = null
    private var droppedFrames = 0L
    private var connectJob: Job? = null
    private var statsJob: Job? = null
    private var audioJob: Job? = null

    /** 连接并开始串流。 */
    fun connect(peer: Peer) {
        disconnect()
        _state.update {
            it.copy(
                phase = StreamPhase.CONNECTING,
                peer = peer,
                message = null,
                droppedFrames = 0L,
            )
        }
        droppedFrames = 0L
        connectJob = viewModelScope.launch {
            val settings = settingsRepo.settings.first()
            val cfg = settings.toStreamCfg()
            streamCfg = cfg
            pacer = FramePacer(cfg.fps)
            _state.update {
                it.copy(
                    resolutionLabel = "${cfg.width}x${cfg.height}",
                    codecLabel = cfg.codec.name,
                    overlayVisible = settings.overlayEnabled,
                )
            }

            val activeDecoder = decoderFactory()
            decoder = activeDecoder
            val sink = StreamSink(activeDecoder)
            val opened = runCatching { client.connect(peer, cfg, sink) }.getOrElse { cause ->
                _state.update {
                    it.copy(phase = StreamPhase.ERROR, message = cause.message ?: "连接失败")
                }
                return@launch
            }
            session = opened
            observeSession(opened)
            if (cfg.audioEnabled) {
                val player = audioPlayerFactory()
                runCatching { player.start(dev.beam.core.audio.PcmAudioFormat()) }
                    .onSuccess { audioPlayer = player }
            }
            opened.state.collect { sessionState ->
                if (sessionState == dev.beam.core.net.SessionState.STREAMING) {
                    _state.update { it.copy(phase = StreamPhase.STREAMING, message = null) }
                }
            }
        }
    }

    /** Surface 就绪：启动解码器并设置刷新率。 */
    fun onSurfaceAvailable(surface: Surface) {
        this.surface = surface
        val cfg = streamCfg ?: return
        val decoder = this.decoder ?: return
        runCatching {
            SurfaceFrameRate.apply(surface, cfg.fps)
            decoder.start(surface, videoFormat(cfg))
        }.onFailure { cause ->
            _state.update { it.copy(phase = StreamPhase.ERROR, message = cause.message) }
        }
    }

    /** Surface 销毁：停止解码器。 */
    fun onSurfaceDestroyed() {
        decoder?.stop()
        surface = null
    }

    /** 断开并释放资源。 */
    fun disconnect() {
        connectJob?.cancel()
        statsJob?.cancel()
        audioJob?.cancel()
        connectJob = null
        statsJob = null
        audioJob = null
        session?.close()
        session = null
        decoder?.stop()
        decoder = null
        audioPlayer?.release()
        audioPlayer = null
        surface = null
        if (_state.value.phase != StreamPhase.IDLE) {
            _state.update { it.copy(phase = StreamPhase.DISCONNECTED) }
        }
    }

    /** 切换统计浮层。 */
    fun toggleOverlay() = _state.update { it.copy(overlayVisible = !it.overlayVisible) }

    /** 展开/收起悬浮工具栏。 */
    fun toggleToolbar() = _state.update { it.copy(toolbarVisible = !it.toolbarVisible) }

    /** 上行输入事件。 */
    fun sendInput(event: InputEvent) {
        runCatching { session?.sendInput(event) }
    }

    /** 请求主机发送新的 IDR。 */
    fun requestIdr() {
        runCatching { session?.requestIdr() }
    }

    // ------------------------------------------------------------- 内部实现

    private fun observeSession(opened: Session) {
        statsJob = viewModelScope.launch {
            opened.events.collect { event ->
                when (event) {
                    is SessionEvent.Stats -> _state.update {
                        it.copy(stats = event.stats, phase = StreamPhase.STREAMING)
                    }
                    is SessionEvent.Reconnecting -> _state.update {
                        it.copy(
                            phase = StreamPhase.RECONNECTING,
                            message = "网络波动，正在重连…",
                        )
                    }
                    is SessionEvent.Disconnected -> _state.update {
                        it.copy(phase = StreamPhase.DISCONNECTED, message = event.message)
                    }
                    is SessionEvent.Error -> _state.update {
                        it.copy(phase = StreamPhase.ERROR, message = event.message)
                    }
                    is SessionEvent.StateChanged -> Unit
                }
            }
        }
        audioJob = viewModelScope.launch {
            opened.audioPackets.collect { packet ->
                val player = audioPlayer
                if (player != null) {
                    player.enqueue(packet.data, packet.ptsUs)
                }
            }
        }
    }

    private fun videoFormat(cfg: StreamCfg): VideoFormat = VideoFormat(
        width = cfg.width,
        height = cfg.height,
        codec = when (cfg.codec) {
            dev.beam.core.net.VideoCodec.H264 -> DecoderCodec.H264
            dev.beam.core.net.VideoCodec.HEVC -> DecoderCodec.HEVC
            dev.beam.core.net.VideoCodec.AV1 -> DecoderCodec.AV1
        },
        fps = cfg.fps,
    )

    /**
     * 帧接收端：把 Rust 回调的帧交给解码器。
     *
     * 帧回调发生在 Rust 派发的专用线程；`FrameSink` 契约规定缓冲在回调返回后失效，
     * 因此「等待节拍」分支必须**先拷贝**再延迟提交。
     */
    private inner class StreamSink(
        private val decoder: VideoDecoder,
    ) : FrameSink {

        override fun onFrame(buffer: ByteBuffer, ptsUs: Long, captureTsUs: Long, key: Boolean) {
            val pacer = pacer ?: return
            when (val decision = pacer.decide(ptsUs, nowUs())) {
                is PacerDecision.Present -> runCatching { decoder.queue(buffer, ptsUs, key) }
                is PacerDecision.Drop -> {
                    droppedFrames++
                    _state.update { it.copy(droppedFrames = droppedFrames) }
                }
                is PacerDecision.Skip -> {
                    droppedFrames++
                    _state.update { it.copy(droppedFrames = droppedFrames) }
                }
                is PacerDecision.Wait -> {
                    // 提前到达：拷贝后按节拍延迟提交（等待分支罕见，拷贝成本可接受）。
                    val copy = ByteBuffer.allocate(buffer.remaining())
                    copy.put(buffer.duplicate())
                    copy.flip()
                    viewModelScope.launch {
                        delay(decision.dueInUs / 1000)
                        runCatching { decoder.queue(copy, ptsUs, key) }
                    }
                }
            }
        }

        override fun onEvent(event: SessionEvent) = Unit
    }

    companion object {

        /** 客户端单调时钟（微秒）。 */
        fun nowUs(): Long = System.nanoTime() / 1000

        /** 手动构造函数注入用的工厂（不引入 DI 框架）。 */
        fun factory(
            client: BeamClient,
            settingsRepo: SettingsRepo,
            decoderFactory: () -> VideoDecoder = { MediaCodecVideoDecoder() },
            audioPlayerFactory: () -> AudioPlayer = { LowLatencyAudioPlayer() },
        ): ViewModelProvider.Factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T =
                StreamViewModel(client, settingsRepo, decoderFactory, audioPlayerFactory) as T
        }
    }
}
