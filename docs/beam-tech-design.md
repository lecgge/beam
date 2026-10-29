# Beam 串流 技术设计文档 v0.1

> 日期:2026-10-29 · 依据:《Beam 串流:Windows + Android 产品与技术方案》Page · 状态:草案
> 所有耗时数字均为设计预算,不是实测;外部数据只沿用方案页已列来源。

## 1. 范围、术语、目标

### 1.1 文档范围
覆盖 Beam 主机(Windows)、Windows 客户端、Android 客户端(手机/平板/TV)的模块划分、接口、协议、构建发布、测试与工作拆解。不覆盖:官网、账号体系、云中继运营。

### 1.2 术语
| 术语 | 含义 |
|---|---|
| 主机 Host | 运行游戏、抓屏编码的 Windows PC |
| 客户端 Client | 解码显示并回传输入的设备(Windows / Android) |
| 会话 Session | 一次从建连到断开的串流过程 |
| 帧组 FEC Group | 同一视频帧的数据分片 + 冗余分片 |
| IDR / Intra-refresh | 关键帧 / 渐进式帧内刷新 |
| E2E | 端到端延迟:主机抓屏时刻 → 客户端上屏时刻 |
| 档位 Tier | T1=4K60,T2=1440p165,T3=1080p200 |
| perf-stats | 仅 Debug 构建的性能统计模块 |

### 1.3 目标 / 非目标
| 阶段 | 做 | 不做 |
|---|---|---|
| MVP | Windows 主机(WGC+NVENC/AMF/QSV HEVC、SendInput 键鼠);Windows/Android 客户端硬解显示;QUIC 视频 datagram + 控制 stream;mDNS;扫码/PIN 配对;1080p60 局域网 | 音频、手柄、虚拟显示器、FEC 自适应、GameStream 兼容、外网 |
| v1 | 音频 Opus、ViGEm 手柄、虚拟显示器、FEC + 自适应码率、断线重连、Android TV、GameStream 兼容层、T1 达标,T2/T3 在 NVENC + Windows 客户端达标、perf-stats | HDR、AV1 默认、多显示器、文件传输 |
| v2 | HDR、AV1、120fps+ Android、多显示器、剪贴板、WOL、Tailscale 引导/中继 | 云游戏托管、macOS/Linux 主机、iOS 客户端(另立项) |

明确不做:账号/云同步、录屏直播、远程办公级文件管理、对反作弊游戏的驱动级输入(v2 再评估)。

## 2. 仓库与工程结构
```
beam/
├─ Cargo.toml                # workspace
├─ crates/
│  ├─ beam-proto/            # 消息、帧头、版本协商(no_std 友好)
│  ├─ beam-net/              # quinn、分包、FEC、码率控制、抖动缓冲
│  ├─ beam-pair/             # 证书、扫码载荷、SPAKE2 PIN、信任库
│  ├─ beam-discovery/        # mdns-sd
│  ├─ beam-session/          # 会话状态机(主/客两侧)
│  ├─ beam-capture/          # WGC / DXGI (windows only)
│  ├─ beam-encode/           # ffmpeg-next: NVENC/AMF/QSV (windows only)
│  ├─ beam-audio/            # WASAPI loopback + Opus 编码;客户端 Opus 解码
│  ├─ beam-input/            # SendInput、ViGEm (windows only) + 输入事件模型
│  ├─ beam-decode/           # Windows 客户端 D3D11VA 解码
│  ├─ beam-render/           # Windows 客户端 D3D11 交换链
│  ├─ beam-config/           # 配置读写、优先级合并、存储
│  ├─ beam-log/              # tracing 初始化、日志轮转
│  ├─ beam-perf/             # perf-stats(feature = "perf-stats")
│  ├─ beam-host/             # 主机编排
│  ├─ beam-client/           # Windows 客户端编排
│  └─ beam-ffi/              # uniffi 导出给 Android
├─ apps/desktop/             # Tauri 2
│  ├─ src/                   # 前端 (React + TS)
│  │  ├─ pages/ (Home, Devices, Settings, Connect, Perf)
│  │  ├─ components/ ipc/ stores/ i18n/
│  └─ src-tauri/ (commands.rs, events.rs, tray.rs, main.rs)
├─ android/
│  ├─ app/  feature-discovery/  feature-pairing/  feature-stream/  feature-settings/
│  ├─ feature-perf/          # 仅 debug 依赖
│  ├─ core-decoder/ core-renderer/ core-input/ core-audio/ core-data/
│  └─ core-net/              # uniffi 生成的 Kotlin + jniLibs/libbeam_ffi.so
├─ drivers/                  # 虚拟显示器(IddCx)与安装脚本(v1)
├─ docs/  tools/ (bench, pcap 解析)  .github/workflows/
```

## 3. 模块设计(统一模板)
每节字段:职责|边界(负责/不负责/依赖/被依赖/禁止)|接口|数据结构|线程模型|错误码|配置|性能预算(T1/T2/T3)|测试|里程碑

### 3.1 beam-proto
- 职责:所有线上消息与帧头的定义、序列化(postcard)、版本协商。
- 边界:只含类型与编解码;不做 IO。依赖 serde/postcard;被 net/session/ffi 依赖;禁止依赖任何平台 crate 或 tokio。
- 接口:`fn encode<M: Message>(m:&M, buf:&mut BytesMut)`;`fn decode(buf:&[u8]) -> Result<AnyMsg, ProtoError>`;`fn negotiate(local:&Caps, remote:&Caps) -> Result<Agreed, ProtoError>`
- 数据:`Hello{ver,caps}`、`Caps{codecs,max_res,max_fps,features}`、`StreamConfig`、`InputEvent`、`StatsReport`、`VideoPktHeader`(见第 5 章)。
- 线程:纯函数。 错误码:`E1xxx`(1001 版本不兼容,1002 解码失败,1003 未知消息)。
- 配置:无。 性能:单消息编解码 < 2 µs。 测试:proptest 往返、模糊测试 cargo-fuzz、版本兼容快照。 里程碑:MVP。

### 3.2 beam-net
- 职责:quinn 端点;视频/音频 datagram 分包与重组;Reed-Solomon FEC;码率控制(基于丢包与 RTT 的 GCC 简化版);接收端抖动缓冲;连接统计。
- 边界:不懂编码格式,只搬字节;不做配对决策(TLS 校验回调由 pair 提供)。依赖 proto、pair(验证器 trait)、quinn、reed-solomon-erasure;被 session、ffi 依赖。禁止依赖 capture/encode/decode。
- 接口:
```rust
pub struct NetEndpoint; impl NetEndpoint {
  pub async fn listen(cfg: NetConfig, verifier: Arc<dyn PeerVerifier>) -> Result<Self, NetError>;
  pub async fn connect(addr: SocketAddr, cfg: NetConfig, verifier: Arc<dyn PeerVerifier>) -> Result<Conn, NetError>; }
pub trait VideoSender { fn send_frame(&self, f: EncodedFrame) -> Result<(), NetError>; }
pub trait VideoReceiver { async fn next_frame(&mut self) -> Result<AssembledFrame, NetError>; }
pub struct RateController { pub fn on_feedback(&mut self, fb: &Feedback) -> BitrateDecision; }
```
- 数据:`EncodedFrame{frame_id,pts_us,capture_ts_us,is_key,slices:Vec<Bytes>}`、`AssembledFrame`、`Feedback{loss,rtt,recv_rate}`。
- 线程:tokio 多线程运行时;发送路径专用任务,UDP GSO 批量发包;接收重组单任务;FEC 解码在 rayon 或 blocking 线程。
- 错误码:`E2xxx`(2001 连接超时,2002 证书拒绝,2003 datagram 不支持,2004 帧过期丢弃)。
- 配置:`fec_ratio`(默认 0.2,码率>150 Mbps 时下调到 0.1)、`max_datagram=1200`、`jitter_frames`(有线 0,Wi-Fi 自适应 0–1)、`bitrate_min/max`。
- 性能:分包+FEC 编码 T1 ≤0.8 ms、T2 ≤0.4 ms、T3 ≤0.3 ms;重组+FEC 恢复同量级。
- 测试:丢包/乱序/抖动注入(自写 UDP 代理)、FEC 恢复率、码率控制仿真。 里程碑:MVP(无 FEC)/v1(FEC+码率控制+抖动缓冲)。

### 3.3 beam-pair
- 职责:Ed25519 自签证书(rcgen);信任库;二维码载荷 `beam://pair?h=ip:port&fp=..&t=token`;SPAKE2 PIN;实现 rustls 校验器。
- 边界:不做网络 IO(PIN 交换消息由 session 经 net 传递);依赖 proto、rustls、rcgen、spake2;被 net/session/ffi 依赖;禁止依赖 UI。
- 接口:`Identity::load_or_create(store)`;`fn qr_payload(&self, addr) -> PairPayload`;`PinPairing::start(pin) -> (Self, Msg)`/`finish(msg) -> Result<Fingerprint,_>`;`impl PeerVerifier for TrustStore`。
- 数据:`Fingerprint([u8;32])`、`TrustedPeer{fp,name,platform,paired_at,perms}`。
- 线程:同步 API,在调用方任务内执行。 错误码:`E3xxx`(3001 PIN 错误,3002 次数超限锁定 60 s,3003 token 过期,3004 指纹不匹配)。
- 配置:`pin_len=4`、`pin_ttl=120s`、`max_attempts=3`。 性能:不在热路径。 测试:中间人模拟、错误 PIN、过期 token。 里程碑:MVP。

### 3.4 beam-discovery
- 职责:mDNS 广播 `_beam._udp.local`(TXT:id、name、ver、fp 前 8 位、port)与浏览。
- 边界:只发现不连接;依赖 mdns-sd;被 host/client/ffi 依赖。Android 可选用 NsdManager 兜底(在 Kotlin 层)。
- 接口:`Advertiser::start(info)`;`Browser::start() -> impl Stream<Item=DiscoveryEvent>`。 错误码 `E4xxx`。 配置:`enabled`、`iface_allowlist`。 测试:多网卡、组播被阻断回退手动 IP。 里程碑:MVP。

### 3.5 beam-capture(Windows)
- 职责:WGC(主)/DXGI 复制(回退)抓屏,输出 GPU 上的 D3D11 纹理;设置 `MinUpdateInterval`;光标单独通道。
- 边界:不做颜色转换和编码;不拷到 CPU。依赖 windows-capture、windows crate;被 host 依赖。禁止依赖 net。
- 接口:
```rust
pub trait CaptureSource: Send { fn start(&mut self, cfg: CaptureConfig) -> Result<(), CapError>;
  fn latest(&mut self) -> Option<CapturedFrame>; /* 非阻塞,取最新 */ fn stop(&mut self); }
pub struct CapturedFrame { pub tex: ID3D11Texture2D, pub ts_us: u64, pub dirty: bool }
```
- 线程:WGC 回调线程只更新“最新帧”槽(单槽交换);编码节拍线程按目标帧率拉取。
- 错误码 `E5xxx`(5001 无显示器,5002 独占全屏无法抓取→切 DXGI,5003 设备丢失)。 配置:`backend=auto|wgc|dxgi`、`display_id`、`min_update_interval`。
- 性能:抓屏+取纹理 T1 ≤1.5 ms、T2/T3 ≤1 ms;必须能交付 ≥ 目标帧率(主机显示器刷新率需 ≥ 目标)。
- 测试:帧率计数(165/240Hz 显示器)、设备丢失恢复、多显示器。 里程碑:MVP(WGC)/v1(DXGI 回退、虚拟显示器)。

### 3.6 beam-encode(Windows)
- 职责:GPU 上 RGB→NV12(D3D11 VideoProcessor 或 compute shader);NVENC/AMF/QSV 硬编(ffmpeg-next 封装,D3D11 硬件帧上下文);低延迟参数;切片输出;强制 IDR;码率热更新。
- 边界:不负责发包;依赖 ffmpeg-next(LGPL 动态库)、windows;被 host 依赖。禁止依赖 net/session。
- 接口:`trait VideoEncoder { fn encode(&mut self, f:&CapturedFrame) -> Result<EncodedFrame, EncError>; fn request_idr(&mut self); fn set_bitrate(&mut self,bps:u32); }`;`fn probe() -> Vec<EncoderCaps>`。
- 参数基线:NVENC P1 + ultra-low-latency、CBR、无 B 帧、无 lookahead、VBV=1 帧、intra-refresh;Ada 上 T1 及高帧率档开启 SFE;AMF `ultralowlatency`+`vbr_latency`;QSV `veryfast`。
- 线程:单独编码线程(与节拍线程合并),同步调用。 错误码 `E6xxx`(6001 无可用硬编,6002 会话数超限,6003 参数不支持→降档)。
- 配置:`encoder=auto|nvenc|amf|qsv`、`codec=hevc|h264|av1`、`slices`、`sfe`。
- 性能:颜色转换+编码 T1 ≤7 ms(Ampere 级,Ada ≤4 ms)、T2 ≤3 ms、T3 ≤2 ms;AMF/QSV 的 T2/T3 为实验档。
- 测试:probe 矩阵、各档 P99 编码耗时基准、码率切换无花屏。 里程碑:MVP / v1(切片、SFE)。

### 3.7 beam-audio
- 职责:主机 WASAPI loopback 48 kHz → Opus 5 ms 帧;Windows 客户端 Opus 解码 + WASAPI 输出;Android 由 core-audio 解码(可复用 ffi 中的 Opus 解码)。
- 边界:不做混音/回声消除。错误码 `E7xxx`。配置:`audio_mode=host+stream|stream_only|off`、`bitrate=128k`。性能:采集→编码 ≤6 ms,端到端目标 ≤40 ms。测试:音画同步偏差 <30 ms。里程碑:v1。

### 3.8 beam-input
- 职责:输入事件模型(在 proto 定义);主机侧 SendInput 注入键鼠、vigem-client 虚拟 Xbox 手柄、震动回传;权限控制(按设备)。
- 边界:不采集客户端输入(Windows 客户端由 beam-client 采集,Android 由 core-input)。禁止依赖 capture/encode。
- 接口:`trait InputSink { fn apply(&mut self, ev: InputEvent) -> Result<(), InputError>; }`;`GamepadHub::attach(slot) -> GamepadHandle`。
- 线程:输入 stream 接收任务 → 有界通道 → 注入线程(高优先级)。错误码 `E8xxx`(8001 ViGEmBus 未安装,8002 权限拒绝)。性能:接收→注入 ≤1 ms。测试:回放脚本、手柄热插拔。里程碑:MVP(键鼠)/v1(手柄)。

### 3.9 beam-session
- 职责:主/客会话状态机;串联 capture→encode→net、net→decode→render;能力协商、IDR 请求、重连、统计汇总。
- 状态:`Idle → Discovering → Pairing → Connecting → Negotiating → Streaming ⇄ Reconnecting → Closed`;`Error` 可从任意状态进入。
- 接口:`HostSession::start(cfg, deps) -> SessionHandle`;`ClientSession::connect(peer, cfg) -> SessionHandle`;`SessionHandle::{events()->Stream<SessionEvent>, update(StreamConfig), stop()}`。
- 边界:不直接调用平台 API,通过 trait(CaptureSource/VideoEncoder/VideoSender/…)注入,便于测试。被 host/client/ffi 依赖。
- 错误码 `E9xxx`。配置:`reconnect_timeout=30s`、`stall_threshold=3s`。测试:状态机表驱动测试 + 假实现集成测试。里程碑:MVP(无重连)/v1。

### 3.10 beam-decode / beam-render(Windows 客户端)
- 职责:ffmpeg D3D11VA 硬解 HEVC/H.264(/AV1 v2);解码纹理直接交给 D3D11 flip-model 交换链,支持可变刷新率;叠加层由 Tauri 窗口负责。
- 接口:`trait VideoDecoder { fn decode(&mut self, f: AssembledFrame) -> Result<DecodedFrame, DecError>; }`;`Renderer::present(&DecodedFrame)`。
- 线程:解码线程 → 单槽 → 渲染线程(按 vblank)。错误码 `E10xxx`。性能:解码 T1 ≤8、T2 ≤5、T3 ≤4 ms;上屏等待平均半帧。测试:各显卡解码基准、丢帧统计。里程碑:MVP(简版)/v1。

### 3.11 host-app / client(Tauri,apps/desktop)
- 职责:UI、托盘、通知;只通过 commands/events 与 Rust 交互;视频帧不经 IPC。
- Commands:`get_host_status`、`start_hosting(opts)`、`stop_hosting`、`get_pairing_code`、`approve_pair(id)`、`reject_pair(id)`、`remove_device(id)`、`list_hosts`、`connect(host_id)`、`disconnect`、`get_settings`、`set_settings(patch)`;debug 下另有 `perf_snapshot`、`perf_export(format)`。
- Events:`host://state-changed`、`pair://request`、`pair://code-rotated`、`discovery://updated`、`session://stats`(500 ms)、`session://reconnecting`、`session://disconnected`、`perf://tick`(debug,250 ms)。
- 错误:command 返回 `{code:"E2001", message, retryable}`,前端按 code 显示中文文案。里程碑:MVP。

### 3.12 Android 模块
| 模块 | 职责 | 对外接口(Kotlin) | 禁止 |
|---|---|---|---|
| :app | 导航、主题、权限、TV 入口 | — | 业务逻辑 |
| :feature-discovery | 主机列表、NSD 兜底 | `DiscoveryViewModel` | 直接调 JNI(经 core-net) |
| :feature-pairing | CameraX+ML Kit 扫码、PIN 键盘 | `PairingViewModel` | — |
| :feature-stream | SurfaceView、悬浮工具栏、统计浮层 | `StreamScreen` | 解码细节 |
| :feature-settings | 设置页 | — | — |
| :feature-perf | 性能面板(仅 debugImplementation) | `PerfPanel` | 被 release 依赖 |
| :core-net | uniffi 绑定 `BeamClient`、回调转 Flow | `interface BeamClient { suspend fun connect(peer: Peer, cfg: StreamCfg): Session }` | — |
| :core-decoder | MediaCodec 异步模式、`KEY_LOW_LATENCY`、厂商 low_latency 解码器、output fence | `interface VideoDecoder { fun start(surface: Surface, fmt: VideoFormat); fun queue(frame: ByteBuffer, ptsUs: Long, key: Boolean); fun stop() }` | 网络 |
| :core-renderer | Surface 管理、`Surface.setFrameRate`、帧节奏 | `FramePacer` | — |
| :core-input | 触控/手柄/遥控映射为 InputEvent | `InputMapper` | — |
| :core-audio | Opus 解码 + AAudio | `AudioPlayer` | — |
| :core-data | DataStore 设置、信任主机 | `SettingsRepo`、`HostRepo` | — |
- uniffi 导出(beam-ffi):`BeamClient::new(config)`、`browse(listener)`、`pair_qr(payload)`、`pair_pin(peer,pin)`、`connect(peer,cfg,sink: FrameSink)`;`callback interface FrameSink { on_frame(data: Vec<u8>/ByteBuffer, pts_us, capture_ts_us, key) ; on_event(SessionEvent) }`。热路径用 JNI 直接 ByteBuffer(DirectByteBuffer)避免 uniffi 拷贝;控制面走 uniffi。
- 线程:Rust tokio 运行时在 so 内;帧回调在专用线程 → `queue()`;UI 在 Main。性能:解码 T1 ≤8、T2 ≤5 ms(T3 Android 不作为验收目标)。里程碑:MVP(列表/配对/解码)/v1(TV、手柄、音频)。

### 3.13 beam-perf(perf-stats,仅 Debug)
- 职责:各阶段耗时直方图(抓屏/颜色转换/编码/分包/RTT/丢包/FEC 恢复/抖动缓冲/解码/渲染/E2E)、码率、帧率、掉帧、CPU/GPU/内存;10 分钟环形缓冲;CSV/JSON 导出。
- 门控:cargo feature `perf-stats`(默认关,release 禁止开启,CI 检查);采集宏 `perf_span!("encode")` 在 feature 关闭时展开为空。Android `debugImplementation(project(":feature-perf"))` + `BuildConfig.PERF_STATS`;release 构建 R8 后校验无 perf 类。
- 接口:`Perf::record(stage, dur)`、`Perf::snapshot() -> PerfSnapshot`、`Perf::export(fmt, path)`。E2E 测量:NTP 式四时间戳时钟同步(每 5 s)+ 回环上屏时间戳交叉校验 + 可选光学实测。
- 性能:采集开销 < 1% CPU,单点 < 200 ns。里程碑:v1(M2)。

### 3.14 beam-config / storage
- 职责:配置模型、校验、持久化(Windows `%APPDATA%/Beam/config.toml`,信任库 `trust.json`,私钥 DPAPI 加密);Android 用 DataStore + Keystore。
- 优先级:命令行/环境变量 > 会话内临时覆盖 > 用户配置文件 > 设备能力推导默认值 > 内置默认。错误码 `E11xxx`。里程碑:MVP。

### 3.15 beam-log
- tracing + tracing-subscriber,JSON 文件轮转(7 天/50 MB);字段规范:`session_id`、`peer`、`stage`、`code`;禁止记录 PIN、私钥、token;热路径只允许 `trace!` 且默认关闭。Android 侧 Logcat + 文件。里程碑:MVP。

## 4. 依赖与分层
```mermaid
graph TD
  subgraph L0 基础
    proto; config; log
  end
  subgraph L1 能力
    net; pair; discovery; capture; encode; audio; input; decode; render; perf
  end
  subgraph L2 编排
    session
  end
  subgraph L3 应用
    host[beam-host]; client[beam-client]; ffi[beam-ffi]
  end
  subgraph L4 外壳
    tauri[apps/desktop]; android[Android core-net]
  end
  net-->proto; net-->pair; pair-->proto; discovery-->proto
  session-->net; session-->proto; session-.trait.->capture; session-.trait.->encode; session-.trait.->decode
  host-->session; host-->capture; host-->encode; host-->audio; host-->input; host-->discovery
  client-->session; client-->decode; client-->render
  ffi-->session; ffi-->net; ffi-->pair; ffi-->discovery
  tauri-->host; tauri-->client; android-->ffi
```
规则:只允许上层依赖下层;L1 之间仅 net→pair 允许;session 只依赖 trait;平台 crate(capture/encode/input/decode/render)只能被 host/client 引用,不得进入 ffi;perf 可被任意层以 feature 可选依赖。

### 4.1 时序
```mermaid
sequenceDiagram
  participant C as 客户端
  participant H as 主机
  H-->>C: mDNS 广播 _beam._udp
  C->>H: QUIC 握手(TLS1.3,首次未信任)
  C->>H: PairRequest(token 或 SPAKE2 msg)
  H->>H: 用户确认 / PIN 校验
  H-->>C: PairOk(双方固定指纹)
  C->>H: Hello{ver,caps}
  H-->>C: HelloAck{agreed StreamConfig}
  H-->>C: 视频 datagram(首帧 IDR)
  C->>H: 输入 stream / Feedback
  Note over C,H: 3 s 无帧 → Reconnecting
  C->>H: 新连接/迁移 + Resume{session_id}
  H-->>C: IDR
  C->>H: Bye
```
状态机:Idle→Connecting→(Pairing)→Negotiating→Streaming;Streaming→Reconnecting(3 s 无数据)→Streaming(30 s 内恢复)|Closed(超时);任意→Error→Closed。

## 5. 协议规格
- ALPN `beam/1`;控制 stream 0(双向可靠),输入 stream(单向可靠,客户端→主机),视频/音频走 QUIC datagram。
- 控制消息:Hello/HelloAck、PairRequest/PairChallenge/PairOk/PairFail、StreamConfig、RequestIdr、SetBitrate、Feedback、ClockSync(t0..t3)、StatsReport、Resume、Bye、Error{code}。
- 帧头(视频 datagram,16 字节,大端):
| 字段 | 位宽 | 说明 |
|---|---|---|
| ver | 4 | 当前 1 |
| type | 4 | 0 视频 1 音频 2 FEC 冗余 |
| flags | 8 | bit0 关键帧,bit1 帧末片,bit2 切片边界 |
| frame_id | 32 | 递增 |
| shard_idx | 16 | 组内分片号 |
| data_shards | 8 | k |
| fec_shards | 8 | m |
| capture_ts | 48 | 主机微秒时钟 |
| payload | ≤1184 B | — |
- FEC:按帧 Reed-Solomon GF(2^8),k ≤ 200,m = ceil(k×ratio);默认 ratio 0.2,码率 >150 Mbps 降到 0.1,丢包 >2% 升至 0.3;单帧超过 255 片时拆成多个组。
- 版本协商:Hello 带 `proto_ver` 和 `features` 位图;取双方最大公共主版本;主版本不同直接拒绝(E1001),次版本向后兼容(未知字段忽略,postcard 末尾追加)。
- 兼容策略:v1 GameStream 兼容层作为独立 crate `beam-gamestream`,只按公开协议行为实现,不参考 GPL 代码。

## 6. 跨模块约定
- 时间:所有时间戳为单调时钟微秒 `u64`;主机抓屏时刻为帧的 capture_ts;客户端通过 ClockSync 求偏移。
- 线程:tokio 只用于网络与控制;抓屏/编码/解码/渲染/注入用专用 OS 线程并提升优先级(Windows MMCSS "Games");热路径禁止阻塞锁,使用单槽交换或有界 SPSC。
- 内存所有权:D3D11 纹理用引用计数 COM 指针,capture→encode 同一设备上零拷贝;EncodedFrame 用 `Bytes` 共享切片;JNI 热路径 DirectByteBuffer 由 Rust 侧池化,回调返回后归还。
- 错误传播:各 crate 定义 thiserror 枚举并映射到 `E` 码;session 汇总为 `SessionEvent::Error{code, retryable}`;UI 只根据 code 显示文案。
- 日志:见 3.15。配置优先级:见 3.14。

## 7. 构建与发布
- Cargo features:`perf-stats`(仅 debug)、`gamestream`(v1)、`av1`(v2)、`vigem`(默认开)。release profile:`lto="thin"`、`panic="abort"`。
- Android:buildType debug/release;flavor `store`/`internal`;`:feature-perf` 只在 debug;cargo-ndk 构建 arm64-v8a(+ x86_64 给模拟器)。
- CI(GitHub Actions):fmt/clippy/test(Linux 跑纯逻辑 crate,Windows runner 跑平台 crate 编译);Android assemble + 单测;release 产物检查无 perf 符号;cargo-deny 许可证检查。
- 签名:Windows 安装包与驱动代码签名;Android release keystore 放 CI secret。
- FFmpeg:只用 LGPL 构建(不开 `--enable-gpl` 与 nonfree),以 DLL 动态链接,随包附许可证与源码获取说明。
- 许可证:自有代码 MIT/Apache-2.0;禁止复制、翻译或让 AI 改写 Sunshine/Moonlight 的 GPL 代码;NVENC/AMF/QSV SDK 头文件分发前逐个核对;不使用 NVIDIA 商标作产品名。

## 8. 测试与验收
- 单元:proto 往返/模糊、FEC、状态机、配置合并。集成:假 capture/encoder 在本机回环跑完整会话;丢包注入。
- 性能基准(有线千兆,P99,持续 10 分钟):
| 档位 | E2E 目标 | 掉帧率 | 码率 | 验收客户端 |
|---|---|---|---|---|
| T0 1080p60 | ≤30 ms | <0.5% | 20–40 Mbps | Windows、Android |
| T1 4K60 | ≤27 ms | <0.5% | 80–150 Mbps | Windows;Android 旗舰(降为屏幕分辨率) |
| T2 1440p165 | ≤14 ms | <1% | 100–200 Mbps | Windows 165Hz 屏;165Hz Android 平板 |
| T3 1080p200 | ≤11 ms | <1% | 80–150 Mbps | Windows 240Hz 屏 |
- 测试矩阵:GPU = RTX 20/30/40 系各 1、RX 6000/7000 各 1、Intel Arc 或带 QSV 核显 1;Android = 骁龙 8 Gen 2/Gen 3 手机、165Hz 平板(如拯救者 Y700 2025)、中端天玑手机、Android TV 盒子 1;OS = Windows 11 24H2、Windows 10 22H2(兼容)。

## 9. 工作拆解
| 里程碑 | 任务 | 工时(人日) | 依赖 | 可并行 |
|---|---|---|---|---|
| M0 原型(第 1 天) | proto 基础类型;net 无 FEC datagram;capture WGC;encode NVENC;client decode/render 简版 | 1 | — | capture/encode 与 net 并行 |
| M1 MVP(第 2–3 天) | pair(QR+PIN);discovery;Tauri 主页/设备/设置;Android 列表/配对/解码;input 键鼠 | 2 | M0 | Tauri UI 与 Android 并行 |
| M2 可日用(第 2 周) | 音频;FEC;重连状态机;延迟浮层;perf-stats;config/log 完善 | 5 | M1 | 音频与 FEC 并行 |
| M2.5 高帧率(第 3 周) | 零拷贝颜色转换;切片/SFE;帧节奏+MinUpdateInterval;Android 低延迟解码与高刷;T1–T3 基准 | 5 | M2(perf-stats) | Windows 与 Android 并行 |
| M3 v1(第 3–5 周) | ViGEm 手柄;虚拟显示器驱动;Android TV;GameStream 兼容层;安装包与签名 | 10 | M2 | 驱动、TV、兼容层三线并行 |
| M4 v2(第 6 周起) | HDR/AV1/120fps+;外网;机型适配矩阵 | 待定 | M3 | — |
工时为单人 + AI 辅助编码的粗估。

风险与对策:
| 风险 | 对策 |
|---|---|
| ViGEmBus 停更 | 手柄做可选组件,评估 VHF/UMDF 自研 |
| 虚拟显示器驱动签名 | IddCx 自研,早期测试签名 |
| WGC 24H2 默认约 60fps | 设置 MinUpdateInterval + DXGI 回退 + perf-stats 监控 |
| AMF/QSV 高帧率不稳 | T2/T3 先只开 NVENC |
| Android 刷新率/游戏助手锁帧 | 按上报刷新率限档并提示 |
| 国产机 MediaCodec 差异 | 解码器白名单 + 自动探测 |
| uniffi 热路径拷贝开销 | 帧数据走 JNI DirectByteBuffer |
| Wi-Fi 高码率不稳 | 有线优先,Wi-Fi 自动限 120 Mbps 并开抖动缓冲 |

## 参考来源(沿用方案页)
- https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-application-note/index.html
- https://arxiv.org/html/2605.01187v1
- https://developer.nvidia.com/blog/video-encoding-at-8k60-with-split-frame-encoding-and-nvidia-ada-lovelace-architecture/
- https://arxiv.org/html/2511.18688
- https://github.com/GPUOpen-LibrariesAndSDKs/AMF/issues/593
- https://www.mintlify.com/LizardByte/Sunshine/configuration/audio-video
- https://github.com/robmikh/Win32CaptureSample/issues/92
- https://github.com/microsoft/Windows.UI.Composition-Win32-Samples/issues/142
- https://www.reddit.com/r/MoonlightStreaming/comments/1i9by7o/moonlight_latency_optimizations/
