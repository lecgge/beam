# Beam 串流项目 文档 vs 实现 差距清单

> 依据：`docs/beam-tech-design.md` v0.1（权威）与 `docs/beam-product-plan.md`
> 仓库：`C:\Users\Wisdplat_ZC_0044\Documents\work\beam`
> 生成日期：2026-10-29

---

## 0. 这份清单是什么

把技术设计文档的**每一条可验证要求**（接口签名、数据结构字段、配置项、错误码、性能预算、线程模型、里程碑、边界与禁止项）逐条对到仓库里的实际代码，判定为 **已实现 / 占位 / 缺失 / 偏差 / 超出** 五档。

**判定口径（关键）**：

| 判定 | 含义 |
|---|---|
| 已实现 | 有真实实现，不是返回错误码、不是 `todo!()`、不是空实现 |
| 占位 | 接口/结构体/函数存在，但实现体是桩（返回错误码、空实现、固定假值） |
| 缺失 | 文档要求的接口或功能在代码里找不到 |
| 偏差 | 有实现，但与文档规定不一致（签名/语义/字段/错误码/配置项不同） |
| 超出 | 代码里有文档没要求的东西（仅当它造成偏差或风险时列入） |

> **「能编译、单测通过」≠「实现了文档要求」。** 本清单的全部条目均带 `file:line` 证据，且所有引用经脚本逐条校验（文件存在 + 行号在范围内）。

> **两列状态的区别**：「判定」是 **2026-10-29 审计当时**的结论，反映该条要求在审计那一刻的状态；「状态」是 **落地进度**，由 `ledger.json` 维护、随每批交付更新。判定为「缺失/偏差/占位」但状态为「已完成」的条目，表示该差距已被后续批次补上——此时「判定」列保留原样以便追溯，「状态」列才是当前真相。

---

## 1. 总览

| 分片 | 覆盖章节 | 条目 | 已实现 | 占位 | 缺失 | 偏差 | 完成度 |
|---|---|---|---|---|---|---|---|
| beam-proto 与协议规格 | §3.1 / §5 / §6 | 82 | 67 | 1 | 6 | 7 | 67/82 |
| beam-net / beam-pair / beam-discovery | §3.2 / §3.3 / §3.4 | 66 | 47 | 1 | 8 | 10 | 47/66 |
| 平台层 crate（capture/encode/audio/input/decode/render） | §3.5 / §3.6 / §3.7 / §3.8 / §3.10 | 93 | 24 | 31 | 32 | 6 | 24/93 |
| session / host / client / ffi / gamestream | §3.9 / §4.1 / §3.12 uniffi / §2 | 65 | 33 | 4 | 20 | 8 | 33/65 |
| Android 12 模块 | §3.12 / §7 Android / 产品方案 §7 §16 | 109 | 86 | 0 | 9 | 14 | 86/109 |
| Tauri 桌面端 | §3.11 / §2 / 产品方案 §6 §15 §16 | 85 | 54 | 2 | 5 | 22 | 54/85 |
| 横切面（分层 / 构建发布 / 测试验收 / 里程碑） | §2 / §4 / §7 / §8 / §9 / §3.13 / §3.14 / §3.15 | 51 | 21 | 6 | 17 | 7 | 21/51 |
| **合计** | | **551** | **332** | 45 | 97 | 74 | **332/551** |

总体完成度 **332/551 ≈ 60%**。

### 落地进度总览（还剩多少）

逐行统计共 **597** 条。下表把「审计当时的判定」与「当前落地状态」交叉起来——**左三列是当时缺什么，右边是现在做到哪了**。

| 审计判定 | 条数 | 已完成 | 进行中 | 已排期 | 未开始 |
|---|---|---|---|---|---|
| 已实现 | 364 | 12 | 0 | 0 | 352 |
| 占位 | 43 | 35 | 0 | 0 | 8 |
| 缺失 | 111 | 57 | 3 | 0 | 39 |
| 偏差 | 76 | 54 | 0 | 0 | 22 |
| 超出 | 3 | 0 | 0 | 0 | 3 |
| **合计** | **597** | **158** | **3** | **0** | **424** |

- 审计判定为**未达成**（占位+缺失+偏差）：**230** 条
- 其中**已落地**：**161** 条；**仍未排期**：**69** 条

> 「未开始」不代表这一条没人管，而是**还没排进任何一批**。排期后的条目会出现在「落地进度台账」一节，带批次号与验收方式。

---

## 1.1 执行摘要（技术经理视角）

### 一句话判断

> **骨架完整，血肉未生。** 协议层与界面层完成度高，但**把两端连起来的每一个环节**——平台后端、传输帧通道、会话控制循环、JNI 导出——都是占位或空实现。当前仓库是一个**能编译、能跑单测、但还不能串流**的 M-1 骨架。

### 按层次看差距（分布比总量更能说明问题）

| 层 | 章节 | 状态 | 一句话 |
|---|---|---|---|
| L0 协议 | §3.1 / §5 / §6 | **最好** | 16 字节帧头**逐位精确**、15 个控制消息齐备、版本协商与向后兼容都对得上 |
| L0 配置/日志 | §3.13 / §3.14 / §3.15 | 好 | 五层优先级合并、50MB/7天轮转、脱敏都在；**但 perf 体系零接入** |
| L1 网络/配对 | §3.2 / §3.3 / §3.4 | 中 | 类型与算法齐备，**但通道没接通**；FEC 无门控恒开、抖动缓冲只有配置字段 |
| L1 平台 | §3.5 / §3.6 / §3.7 / §3.8 / §3.10 | **最差** | 全部 feature 后占位；原生依赖声明了但 `src/` 里一次都没引用 |
| L2 会话 | §3.9 | 中 | 状态机表驱动做得好（81 组转移全覆盖），**但两个超时阈值没有消费方** |
| L3 编排/FFI | §3.9 / §3.12 | 差 | `beam-ffi` **连 `build.rs` 都没有**，`uniffi` 可选依赖声明了但一次没用 |
| L4 外壳 | §3.11 / §3.12 | 中高 | UI 与 DTO 完整，**但输入/帧/配对三条入口都没接到内核** |

### 三条结构性判断

1. **§9 的 M-1 三个高风险验证点一个都没做。** WGC 出纹理、NVENC 出码流、D3D11VA 解码上屏——§9 明确说「M-1 将它们拆开逐一验证，避免风险全部压在 M0」。现在 `tools/bench` 只有 README 占位。M0 之后的所有排期都建立在这三点之上。
2. **`#![forbid(unsafe_code)]` 架构性地封死了平台层。** 18 个 crate **全部**设了这个属性，其中 5 个需要创建 D3D11 纹理与 COM 指针。COM 指针必须 unsafe 才能创建，**这不是「少写几个函数」，是必须先做一次架构决策**。
3. **整条链路的四个「接头」全是空的。**

| 接头 | 位置 | 现状 |
|---|---|---|
| 传输帧通道 | `beam-net` / `beam-client` 的 `recv_frame`、`send_control` | 空实现，`recv_control` 直接返回 `None` |
| 会话控制循环 | `beam-session` | `Transport::recv_control` **全仓无调用点**，Hello/Pair/RequestIdr 等消息零处理 |
| JNI 导出面 | `crates/beam-ffi` | 无 `#[uniffi::export]`、无 UDL、无 `build.rs`——**即使现在生成也生成不出来** |
| 输入采集回传 | Android `:core-input` | `InputMapper` **零生产调用方**（只有自身与单测） |

### 里程碑真实达成度

| 里程碑 | 文档要求 | 实际 |
|---|---|---|
| M-1 工程骨架 | workspace + 17 crate 骨架 + 三个高风险验证点 | 骨架 ✅ / 验证点 ❌ |
| M0 原型 | 主机→客户端打通抓屏→编码→QUIC→解码 | ❌ 链路不存在 |
| M1 MVP | 配对/发现/Tauri/Android/input | ⚠️ UI 与静态逻辑齐，**链路与内核未通** |
| M2 可日用 | 音频/FEC/重连/浮层/perf-stats | ❌ 骨架级 |
| M2.5 高帧率 | 零拷贝/切片/SFE/帧节奏/T1–T3 基准 | ❌ 未开始 |
| M3 v1 | 手柄/虚拟显示器/TV/GameStream/签名 | ❌ 骨架级 |

### 成本低、收益高的四件事（现有资产已经具备，只差接线）

1. **把 perf 体系接上**：`beam-perf` 自身完成度很高（直方图、10 分钟环形缓冲、CSV/JSON、feature 门控全有），但**没有任何 crate 依赖它**，`perf_span!` 只在它自己内部用。给各层加 `perf-stats` 可选依赖 + 埋点即可。
2. **补三条测试基建**：编解码基准（证明「单消息编解码 < 2 µs」）、cargo-fuzz（§3.1 明确要求）、版本兼容快照（固定线上字节序列）。
3. **补 5 个缺失配置项并给它们消费方**：§3.5 的 `backend`/`display_id`/`min_update_interval` 与 §3.6 的 `slices`/`sfe` 在 `beam-config` 里**完全不存在**，用户改了配置文件也不会有效果。
4. **给 `shard_idx` 语义定稿**：文档写「组内分片号」，实现写的是**帧内全局序号** (`global = g*(k+m)+local`)。这是跨端互操作的隐性坑，越晚改代价越大。

### 需要决策者拍板的三件事

| # | 议题 | 背景 | 影响 |
|---|---|---|---|
| 1 | `forbid(unsafe_code)` 是否放开？ | D3D11 纹理与 COM 指针必须 unsafe | **阻塞整个平台层**，也影响「安全默认值」的质量主张 |
| 2 | FEC 在 MVP 阶段关还是开？ | 文档 §3.2「MVP(无 FEC)」，实现**恒开**且无 feature 门控 | 带宽白占 20%，与文档口径冲突 |
| 3 | `shard_idx` 以文档还是实现为准？ | 见上 | 跨端协议兼容性，改动越晚代价越大 |

### 还有一处文档自身的矛盾需要裁决

§5 同时写了「k ≤ 200」和「单帧超过 255 片时拆成多个组」两条互斥规则。实现按 **200** 拆组（`group_capacity = 200 × max_payload`），255 只用于限制 `k+m`。这是**文档需要统一**，不是代码的问题。

### 第一批 / 第二批落地后的变化（2026-10-29 收口复验）

审计之后已落地两批。下表是**当前状态**，与上面的审计快照对照看：

| 项 | 审计时 | 现在 |
|---|---|---|
| 工作区测试数 | 171 passed | **273 passed**（fmt/check 全绿，0 warning） |
| 四个「接头」 | 全空 | **控制面已通**（真实 QUIC + 自签证书 + 能力协商，本机回环断言两端 `Agreed` 一致）；**帧通道、uniffi 导出面、Android 输入采集仍空** |
| perf 体系 | 零接入 | **能力已补齐**（CPU/GPU/内存承载、NTP 式时钟同步、E2E 交叉校验、43.5ns/点开销基准）；**但仍未被任何 crate 依赖、仍无埋点** |
| 配置项 | §3.5/§3.6 的五项在 beam-config 里完全不存在 | **已补齐并有消费方**（单一真值来源） |
| CI 的 release 无 perf 符号门禁 | **恒通过**（检查对象错） | **真会响**：release 通过 / debug 被抓到，两道门禁均经独立复现 |
| 帧头语义 | shard_idx 实现为帧内全局序号（文档写组内） | 仍未改，待 B2-B 按决策 D3 处理 |
| 平台层 | 全部 feature 后占位 | 仍未动；**工具链已备好**（LGPL FFmpeg 6.1 + libclang + cargo-ndk），等 D1 的局部 unsafe 放开 |

### 三个结构性判断（审计时）

1. **§9 的 M-1 三个高风险验证点一个都没做。** WGC 出纹理、NVENC 出码流、D3D11VA 解码上屏——§9 明确说「M-1 将它们拆开逐一验证，避免风险全部压在 M0」。现在 `tools/bench` 只有 README 占位。M0 之后的所有排期都建立在这三点之上。
2. **`#![forbid(unsafe_code)]` 架构性地封死了平台层。** 18 个 crate **全部**设了这个属性，其中 5 个需要创建 D3D11 纹理与 COM 指针。COM 指针必须 unsafe 才能创建，**这不是「少写几个函数」，是必须先做一次架构决策**。（决策 D1 已定：只放开平台层）
3. **整条链路的四个「接头」全是空的。**

| 接头 | 位置 | 审计时 | 现在 |
|---|---|---|---|
| 传输帧通道 | `beam-net` / `beam-client` 的 `recv_frame`、`send_control` | 空实现 | `send_control`/`recv_control` 已通；**`recv_frame` 仍返回 None**（beam-net 缺非阻塞取帧接口） |
| 会话控制循环 | `beam-session` | `recv_control` 无调用点 | **已落地**：`control.rs` + 两端 runner + 停滞/重连判定 |
| JNI 导出面 | `crates/beam-ffi` | 无 uniffi 导出面、无 build.rs | **仍未动**（需 cargo-ndk，已装好） |
| 输入采集回传 | Android `:core-input` | `InputMapper` 零调用方 | **仍未动** |

---

## 2. 落地进度台账

| 状态 | 条目数 |
|---|---|
| 未开始 | 1 |
| 进行中 | 3 |
| 已完成 | 159 |
| 已阻塞 | 11 |
| 暂不做 | 1 |

### 批次记录

| 批次 | 日期 | 内容 | 范围 | 状态 | 验收方式 |
|---|---|---|---|---|---|
| B1 | 2026-10-29 | 第一批：不依赖任何待决事项的低风险落地 | 测试基建 / perf 指标补全 / 配置项补齐 / CI 门禁修正 | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo test --workspace exit 0（171 → 273 passed）；两道 CI 门禁脚本经主会话独立复现：release 通过 / debug 被抓到 |

- **W1 测试基建** — 写入范围：`crates/beam-proto/**`、`fuzz/**`、`Cargo.toml`；覆盖条目 P1-21, P1-23, P1-24, X8-03
- **W2 perf 补全** — 写入范围：`crates/beam-perf/**`；覆盖条目 X13-06, X13-07, X13-08, X13-09, X13-12
- **W3 配置项补齐** — 写入范围：`crates/beam-config/**`、`crates/beam-capture/**`、`crates/beam-encode/**`；覆盖条目 C-15, C-16, C-17, E-21, E-22
- **W4 CI 门禁** — 写入范围：`.github/workflows/**`、`tools/**`；覆盖条目 X7-12, X7-14, X7-15, X13-14

**B1 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| beam-net::Conn 缺 accept_uni 与 try_next_frame | P5-03 主机侧输入流需要 Conn::accept_uni() -> Result<quinn::RecvStream, NetError>；NetTransport::recv_frame 仍返回 None，需 try_next_frame(&mut self) -> Result<Option<AssembledFrame>, NetError> | B3-B |
| beam-host 未接新配置项 | crates/beam-host/src/runtime.rs 仍写 CaptureConfig { target_fps, ..Default::default() } | 下一批 |
| 桌面端存在平行设置项 | apps/desktop/src-tauri/src/service_crates.rs 的 ui.capture_backend / ui.slices / ui.sfe 与新规范配置项重复 | 下一批 |
| 两套 clock 抽象并存 | crates/beam-session/src/clock.rs 与 crates/beam-perf/src/clock.rs 职责相邻，偏移语义可能分叉 | 下一批 |
| E1003 语义偏差 | 文档 §3.1 写「1003 未知消息」，实现对未知枚举变体返回 E1002，E1003 只覆盖未知消息族标签。已在 snapshot.rs 固化当前行为，文档或实现需择一 | 待裁决 |
| postcard「忽略未知字段」只在尾部追加时成立 | 结构体按字段声明顺序位置化编码；在中间结构体追加字段会静默错解。已在 snapshot.rs 文件头写为警告 | 已记录 |
| Overrides 段字段新增 #[serde(default)] | 属放宽解析而非破坏，主会话复核后保留 | 已复核保留 |
| 自定默认值需产品确认 | MAX_MIN_UPDATE_INTERVAL_MS=1000、MAX_SLICES=16 是兜底上限，文档未规定 | 待确认 |
| apk_no_perf.py 错误信息硬编码「release APK」 | 查 debug APK 时错误信息也写「release APK 的 classes2.dex」 | 下一批 |

| B2 | 2026-10-29 | 第二批：打通控制通道 | beam-client / beam-session 控制面 | 已完成 | cargo check --workspace --all-targets exit 0 / 0 warning；cargo test --workspace exit 0（273 passed）；本机回环集成测试：真实 QUIC 端点 + 自签证书 + TrustStore，Hello→能力协商→HelloAck 两端 Agreed 一致，5 次连跑全绿（0.03–0.04s/次） |

- **B2-A 控制通道** — 写入范围：`crates/beam-client/**`、`crates/beam-session/**`；覆盖条目 T-01, T-02, T-08, T-09, T-14, T-15, T-16, T-17, T-18, T-21, T-22, T-23, P5-02

**B2 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 破坏性 API 变更（3 处） | ① Transport 改为 Transport: ControlChannel；② recv_control 语义变为非阻塞轮询；③ ClientSession::connect_with 不再自动跳到 Streaming，停在 Negotiating 直到真收到 HelloAck | 已记录 |
| 主机侧 QUIC 适配器暂寄居 beam-client | NetTransport::accept 因 beam-host 不在写入范围而放在 beam-client，长期应下沉到 beam-net 或独立 crate | 下一批 |
| 控制流分帧格式是实现约定 | 4 字节大端长度 + postcard + 1 字节族标签，§5 未规定 | 下一批 |
| Bye 会被提前丢弃 | QUIC 最后一个连接引用被释放会立即关连接并丢掉未发出的 Bye；现改为 Bye 后保留传输至句柄释放 | 下一批 |

| B3 | 2026-10-29 | 第三批：平台层真实后端 + 帧语义 + JNI 导出面 | M-1 三个高风险验证点 + beam-net 帧语义 + beam-ffi uniffi | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo test --workspace exit 0（328 passed / 0 failed，40 套件）；真机验证：WGC 抓帧 / DXGI 复制 / flip-model Present / NVENC 出码流 PASS / D3D11VA 硬解 PASS |

- **B3-A 抓屏与渲染** — 写入范围：`crates/beam-capture/**`、`crates/beam-render/**`；覆盖条目 C-01, C-02, C-03, C-04, C-10, C-11, C-12, C-13, C-14, C-24, C-25, V-03, V-04, V-08
- **B3-B beam-net 帧语义与门控** — 写入范围：`crates/beam-net/**`；覆盖条目 P5-25, P5-35, N-28, N-22, P5-03, T-20
- **B3-C uniffi 导出面** — 写入范围：`crates/beam-ffi/**`；覆盖条目 F-04, F-05, F-08, F-09, X2-07
- **B3-D 硬编与硬解** — 写入范围：`crates/beam-encode/**`、`crates/beam-decode/**`；覆盖条目 E-01, E-02, E-03, E-04, E-05, E-06, E-10, E-11, E-12, E-13, E-14, E-15, E-16, E-17, E-18, E-24, V-01, V-02, V-09, V-13

**B3 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 平台层性能与硬件类条目已阻塞 | C-18~C-23、V-10~V-12、E-23/E-25/E-26 需要多显示器 / 165Hz 显示器 / 可热插拔显示设备 / 真实 GPU / Win10 机器，本机无法实测，标为「已阻塞」而非「已完成」 | 需硬件环境 |
| B3-A 需处理 SDK 26100 缺失 | 本机 Windows SDK 仅 19041/22621，MinUpdateInterval 不可用；降级路径必须如实实现并记录 | B3-A |
| B3-D 本机无 GPU 验证环境 | 不得声称 NVENC/D3D11VA 已验证；需提供可复现的验证入口并在文档里标注未验证 | B3-D |
| B3-C 的生成物落地属下一批 | Kotlin 绑定落到 android/core-net/src/main/java/dev/beam/core/net/uniffi/ 与 cargo-ndk 任务，需 android 侧写入权 | 下一批 |
| SetMaximumFrameLatency 在 flip-model 下被 DXGI 拒绝 | 实测返回 DXGI_ERROR_INVALID_CALL，已降级为记日志；延迟上界改由双缓冲保证，「上屏等待平均半帧」未实测（B3-A） | 下一批 |
| 窗口尺寸变化未处理 | beam-render 的交换链按 RenderConfig 固定尺寸创建，窗口 resize 需要 ResizeBuffers，B3-A 未实现；接入 Tauri 窗口前必须补 | B4-B |
| 每帧新建目标纹理 | WGC 帧池是池化的，直接把池化纹理交给编码线程会读到被覆盖的内容；现改为每帧新建目标纹理 + AddRef 交出。代价是每帧一次 D3D11 纹理创建，是否落在 1.5ms/1ms 预算内未实测（B3-A） | 下一批 |
| CaptureConfig 结构体字段一个都没加 | beam-host 用无 ..Default::default() 的字面量构造 CaptureConfig，加字段会破坏 workspace 编译；光标模式因此走 set_cursor_mode()，不能从配置文件派生（B3-A） | 下一批 |
| SessionEvent.Stats 形状不匹配 | B3-C 导出的是嵌套 stats 记录，NativeBeamBridge.mapStats 期望统计字段平铺在变体上 → 统计流会读到 0 值（B3-C） | B4-A |
| connect 内部仍丢弃 cfg/sink | crates/beam-ffi/src/client.rs:196 的 let _ = (cfg, sink); 未动，会话停在 Idle、FrameSink 永不回调（B3-C） | 下一批 |
| uniffi 生成物包名是 uniffi.beam_ffi | NativeBeamBridge 用 Class.forName + 候选名列表找类，需在 Android 侧对齐（B3-C） | B4-A |
| 链接 avcodec 的二进制必须能找到 ffmpeg 的第三方 DLL | conda 版 LGPL ffmpeg 只带自己的 DLL，缺 zlib/dav1d/opus/libxml2/liblzma/iconv/harfbuzz/fontconfig/expat/libpng/pcre2/glib；缺任一即启动 0xC0000135。B3-D 已把它们补进 %TEMP%eam-tools\ffmpeg-lgpl-conda\root\Library\bin。**复现与 CI 必须把 $FFMPEG_DIR\bin 加进 PATH** | 必须跟进 |
| FFmpeg 设备上下文的三个坑（实测） | ①av_hwdevice_ctx_alloc(D3D11VA) 已分配 AVD3D11VADeviceContext，只能填 device 字段、不能覆盖 hwctx 指针（覆盖会让 ffmpeg 用 av_free 释放 Rust 内存→堆破坏）；②free 回调为空→设备引用由我们负责释放；③av_hwdevice_ctx_init 直接解引用 hwctx->device，为 NULL 必崩 | 已写入代码注释 |
| windows-rs 0.62 的 D3D11_VIDEO_PROCESSOR_CAPS 缺字段 | 该结构体少一个 OutputFormatCaps u32，调用 GetVideoProcessorCaps 会越界写栈。**不要调用 GetVideoProcessorCaps** | 已记录 |
| NVENC 有一帧流水线延迟 | 第一次 encode() 拿不到输出。B3-D 新增段内扩展码 E6005（NoOutputYet）表达该状态；但 beam-host 的 poll() 用 .ok() 会静默丢首帧 → 已注入 B4-B 要求显式容忍 E6005 | B4-B |
| D3D11 硬件帧路径的进程退出崩溃 | main 返回后 NVIDIA 驱动侧的第三方 atexit 回调访问已释放对象 → 0xC0000005。示例用 std::process::exit(0) 规避；**库在真实应用退出时仍会遇到**，需排关闭顺序或向驱动侧报 | 下一批 |
| 遗留垃圾目录 C:\Users\Wisdplat_ZC_0044\Documents\beam | B3-D 写错路径建的（1 个文件 crates/beam-encode/src/ffmpeg/mod.rs）。在仓库外不影响构建，但本机回收站通道拒绝工作区外的目标，无法自动清理，需手工删除 | 需手工清理 |
| AMF / QSV 路径无法在本机验证 | 本机 conda LGPL ffmpeg 只编了 NVENC（已扫 avcodec-60.dll 确认无 *_amf / *_qsv）。代码按 §3.6 基线实现完毕，等有对应驱动的机器 | 需对应硬件 |
| 桌面端作为主机未起 QUIC 监听 | beam-host 的 start_quic_session 已就绪，但桌面端 start_hosting 只起 mDNS；因此「主机端人工确认后写信任库」那一侧尚未接通（本批接通的是客户端侧确认） | 下一批 |
| 帧头没有独立 pts 字段 | §5 的 16 字节帧头只有 capture_ts(48 位)，接收端 pts_us = capture_ts_us。若文档要求两者不同，须改 beam-proto/beam-net | 待裁决 |
| 分帧编解码两端各存一份 | §4 不允许 L3 横向依赖（host↔client 同层），所以「4 字节大端长度前缀 + postcard」在 beam-client 与 beam-host 各有一份实现，格式必须保持一致 | 下一批 |
| beam-session 的 HostDeps.input 没有消费方 | runner 从不调用；主机侧输入事件由 beam-host 自己的输入泵驱动。若以后 beam-session 接管会重复注入，需裁决归属 | 待裁决 |
| E2004 缺 SessionEvent 变体 | 目前只能到「日志 + 统计句柄 + 桌面端 dropped_frames」；用 Error 变体会把会话打进 Error 态（丢帧不该致命） | 下一批 |
| 配对链路依赖对端在跑 QUIC | mDNS 只广播指纹前 8 位，信任库需要完整 64 位，故用一次「故意失败」的握手采集证书指纹并与 mDNS 前缀交叉校验；对方只广播不跑 QUIC 时采不到指纹 | 已记录 |
| platform 字段缺失 | §3.4 mDNS TXT 不带平台字段，§5 协议也无设备信息消息 → 信任库里的设备平台恒为「未知平台」。需 beam-discovery 的 TXT 加 plat 或 §5 加一条 DeviceInfo 控制消息 | 待裁决 |

| B4 | 2026-10-29 | 第四批：Android 串流接线 + 帧通道与配置贯通 | android/ uniffi 落地与输入接线；beam-client/beam-host 帧通道与配置；桌面端真实客户端会话 | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo test --workspace exit 0（328 passed）；桌面端 fmt/check exit 0；gradlew assembleDebug BUILD SUCCESSFUL；gradlew test exit 0（22 套件 / 156 次 / 0 失败） |

- **B4-A Android** — 写入范围：`android/**`；覆盖条目 AU-01, AB-04, AU-10, PP-03, AN-19, X2-07, F-11
- **B4-B 帧通道与配置** — 写入范围：`crates/beam-client/**`、`crates/beam-host/**`、`apps/desktop/**`；覆盖条目 P5-03, T-20, E-19, E-20

**B4 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 桌面端作为主机未接 QUIC 监听 | 同上，客户端侧已通、主机侧未通 | 下一批 |
| platform 字段缺失 | 需 beam-discovery TXT 加 plat 或 §5 加 DeviceInfo 控制消息（两者都不在下一批写入范围） | 待裁决 |
| AMF / QSV 路径无法在本机验证 | 本机 conda LGPL ffmpeg 只编了 NVENC（已扫 avcodec-60.dll 确认无 *_amf / *_qsv），代码按 §3.6 基线实现完毕 | 需对应硬件 |
| GPU RGB→NV12（VideoProcessor）未通过 | 本机 CreateVideoProcessorInputView/OutputView 返回 E_INVALIDARG；已验证的替代路径是「NV12 纹理 → D3D11 硬件帧 → NVENC」；文档允许的 compute shader 方案未实现 | 下一批 |

| B5 | 2026-10-30 | 第五批：桌面端主机模式 + 会话层收尾 + 分帧收敛 + perf 埋点接入 | beam-host / beam-session / beam-net / beam-client / beam-perf / 平台层 crate / apps/desktop | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（41 套件 / 350 passed / 0 failed / 5 ignored）；桌面端 fmt / clippy / check 三种 feature 组合均 exit 0 / 0 warning；gradlew assembleDebug assembleRelease exit 0（6 个变体 APK）；gradlew test --rerun-tasks exit 0（22 套件 / 156 次 / 0 失败）；cargo tree -e features 中 perf-stats 0 命中；§7 门禁：rust_no_perf.py（clean 与负样本自检）exit 0、apk_no_perf.py（store+internal release 与 debug 自检）exit 0、门禁单测 19 例全过；android test --rerun-tasks exit 0（22 套件 / 156 次 / 0 失败） |

- **B5-A 桌面端主机模式** — 写入范围：`crates/beam-host/**`、`apps/desktop/**`；覆盖条目 DX-09, CM-02, CM-06
- **B5-B 会话层收尾** — 写入范围：`crates/beam-session/**`；覆盖条目 T-19, S-05, N-18
- **B5-C 分帧编解码单一实现** — 写入范围：`crates/beam-net/**`、`crates/beam-client/**`；覆盖条目 
- **B5-D perf 埋点接入平台层** — 写入范围：`crates/beam-perf/**`、`crates/beam-capture/**`、`crates/beam-encode/**`、`crates/beam-decode/**`、`crates/beam-render/**`；覆盖条目 X13-15

**B5 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 主机侧输入注入器未实现 | 桌面端作为主机时用 beam_host::NoopInputSink 接收输入流（读走但不注入）；beam-input 的 Windows SendInput 分支本身仍是空实现。§4 禁止桌面端直接依赖 beam-input | 下一批 |
| 桌面端 host-platform 未进 CI | apps/desktop/src-tauri 的 host-platform feature（beam-host/platform → WGC + NVENC）本地构建 exit 0，但 CI 的 cargo check 不开它：需要 ffmpeg 开发库，而 §7 要求 LGPL 构建，CI 需引入 lgpl-shared 发行版 | CI 跟进 |
| 分帧编解码的线上字节未双端互验 | beam_net::framing 的固定字节常量只覆盖单端自洽；两端同时改错时往返测试发现不了。已加「主机侧必须与共享实现产出相同字节」的断言，但真正的互验需要独立实现 | 下一批 |

| B6 | 2026-10-30 | 第六批：uniffi 真起会话 + 输入注入真实化 + 错误码文案覆盖 + CI 覆盖平台后端 | beam-session / beam-client / beam-ffi / beam-input / apps/desktop / .github | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（41 套件 / 369 passed / 0 failed / 5 ignored）；cargo check -p beam-ffi --features uniffi exit 0（此前编译不过，Android 的 .so 建不出来）；桌面端 fmt / clippy / check（默认 / host-platform / host-platform-wgc）全部 exit 0 / 0 warning；gradlew :core-net:cargoNdkBuild :core-net:generateUniffiBindings exit 0；gradlew assembleDebug assembleRelease exit 0；gradlew test --rerun-tasks exit 0（22 套件 / 156 次 / 0 失败）；gradlew :core-net:cargoNdkBuild :core-net:generateUniffiBindings exit 0（arm64-v8a + x86_64 两个 ABI）；npm --prefix apps/desktop run build exit 0（typecheck + vite build） |

- **B6-A uniffi connect 真起会话** — 写入范围：`crates/beam-session/**`、`crates/beam-client/**`、`crates/beam-ffi/**`；覆盖条目 F-05
- **B6-B 输入层真实化** — 写入范围：`crates/beam-input/**`；覆盖条目 I-02, I-05
- **B6-C 桌面端错误码文案覆盖** — 写入范围：`apps/desktop/src/**`、`apps/desktop/src-tauri/src/**`；覆盖条目 CM-23, CM-24
- **B6-D CI 覆盖平台后端（主会话自办）** — 写入范围：`.github/workflows/**`、`crates/beam-host/Cargo.toml`、`apps/desktop/src-tauri/Cargo.toml`；覆盖条目 

**B6 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| beam-ffi 的身份与信任库仍未持久化 | BeamClient::new 现场生成自签身份（不落盘），TrustStore 恒为空，对端可信只靠 PeerInfo.fingerprint 固定。需 Android core-data 提供 KeyStore 存储后端后接上 | 下一批 |
| Android 侧键码域与 HID 不匹配 | Android 的 DOM 风格键名（BrowserBack / BrowserHome / ContextMenu）在 USB HID Keyboard page 没有对应 usage，无法编码进 u16。需协议层决定（丢弃 / 带外编码 / 改用 Android KeyEvent 码） | 待裁决 |
| 主机侧输入权限映射未接线 | beam-input 已提供 InputPermissions，但 TrustedPeer.perms / InputSettings → InputPermissions 的映射要写在 beam-host（本批未做） | 下一批 |
| §3.8 的注入线程与 1 ms 指标未落地 | I-10（输入 stream 接收任务 → 有界通道 → 注入线程 + MMCSS Games 优先级）与 I-13（接收→注入 ≤1 ms）仍未做；当前注入在调用方线程同步执行 | 下一批 |
| 桌面端系统通知未做 | CM-19 仍是缺失：需要 tauri-plugin-notification（新依赖），本批未引入 | 下一批 |

| B7 | 2026-10-30 | 第七批：设计规范落地 + Android 状态页 + 输入线程与延迟指标 + Opus 编解码 | apps/desktop/src / android / crates/beam-input / crates/beam-host / crates/beam-audio / tools/ci/check-design-tokens | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0；cargo test -p beam-input exit 0（27 例）；cargo bench -p beam-input --bench inject_latency exit 0（入队→注入 p99 0.9 µs / 预算 1 ms PASS，MMCSS/Games 注册成功）；cargo check/clippy/test -p beam-audio 在默认与 --features opus 两种状态下均 exit 0（opus 态 6 例全过）；npm --prefix apps/desktop run build exit 0；设计规范门禁 exit 0（54 项）；gradlew assembleDebug assembleRelease test --rerun-tasks exit 0（974 任务全执行，28 测试类 / 176 次 / 0 失败） |

- **B7-A 桌面端设计规范** — 写入范围：`apps/desktop/src/**`、`tools/ci/check-design-tokens/**`；覆盖条目 UX-05, UX-06, UX-11, UX-13, UX-14, UX-15, UX-18, UX-19, UX-20, UX-21, UX-22, UX-31, UX-34, UX-35, UX-37, UX-38, UX-41
- **B7-B Android 状态页** — 写入范围：`android/**`；覆盖条目 DS-07, DS-12, DS-19, DS-23, DS-24, DS-27, PP-04, AN-23, AN-28, AN-52
- **B7-C 输入线程与延迟指标** — 写入范围：`crates/beam-input/**`、`crates/beam-host/**`；覆盖条目 I-10, I-13
- **B7-D Opus 编解码（主会话自办）** — 写入范围：`crates/beam-audio/**`；覆盖条目 A-02, A-03

**B7 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| WASAPI loopback 采集与 WASAPI 输出未做 | §3.7 的 A-01（主机侧采集）与 A-04（客户端侧输出）仍是占位：它们要处理设备混音格式协商（设备原生格式常常是 f32，与本 crate 的 i16 交错 PCM 不同） | 下一批 |
| 音频还没进产品链路 | beam-audio 的编解码虽然真的了，但 beam-host / beam-client 都没有调用它 —— 采集→编码→发送→解码→输出这一整条还没接通 | 下一批 |
| 真实 SendInput 注入未自动验证 | 注入有真实副作用（会真的按键），单测与基准都只测「构造 + 入队 + 出队 + 调度」，不含真实 SendInput 调用 | 需真机人工验证 |
| AN-36 / AN-45 未做 | AN-36（Surface 管理归 :core-renderer）要重构；AN-45（:core-audio 用 AAudio 而不是 AudioTrack）要 NDK/JNI 封装 —— 两者都是 v1 范围 | 下一批 |

| B8 | 2026-10-30 | 第八批：网络健壮性与 FEC + 会话层重连状态机 + WASAPI 采集/输出 + 抓屏实测 | crates/beam-net / crates/beam-session / crates/beam-audio / crates/beam-capture / crates/beam-encode / apps/desktop | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0；cargo test -p beam-net exit 0（默认 65 例 / --features fec 73 例）；cargo bench -p beam-net --bench packetize --features fec exit 0（FEC 超预算，数字见 N-24）；cargo test -p beam-session exit 0（54 例）；cargo test -p beam-audio exit 0（默认与 --features opus 双状态）；桌面端 fmt / clippy / check exit 0 / 0 warning |

- **B8-A 网络健壮性与 FEC** — 写入范围：`crates/beam-net/**`；覆盖条目 N-25, N-26, N-16, N-20, N-12, N-13, N-14
- **B8-B 会话层重连与能力协商** — 写入范围：`crates/beam-session/**`；覆盖条目 S-03, S-04, S-06, S-07, S-12, S-19, S-21
- **B8-C WASAPI 采集与输出** — 写入范围：`crates/beam-audio/**`；覆盖条目 A-01, A-04, A-08
- **B8-D 抓屏实测与配置核实（主会话自办）** — 写入范围：`crates/beam-capture/**`、`crates/beam-encode/**`、`apps/desktop/src-tauri/**`；覆盖条目 C-15, C-16, C-17, EV-06

**B8 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| FEC 编码超 §3.2 预算 4.6~13.3 倍 | T1 6.587 ms（预算 0.8）、T2 5.318 ms（预算 0.4）、T3 1.381 ms（预算 0.3）。FEC 仍是 feature 默认关闭（D2），开之前必须优化 | 下一批 |
| 音频 datagram 分包未实现 | 冗余片 type=2 无法表达「属于音频帧」，需 §5 级别的裁决（帧号空间分开 / 帧头腾类型位） | 待裁决 |
| WASAPI 采集/输出真机未验证 | A-01/A-04 的代码与纯逻辑单测（15 例）都在，但 loopback 能否稳定拿到 5 ms 帧、f32 混音格式转换听感、IsFormatSupported 在真实设备上的返回值都没实测。真机测试已写好（默认 #[ignore] + BEAM_AUDIO_DEVICE_TEST=1 门控）但未跑 | 需真机人工验证 |
| 音频还没进产品链路 | beam-audio 的采集/编解码/输出都真了，但 beam-host / beam-client 都没有调用它 —— 采集→编码→发送→解码→输出整条未通 | 下一批 |
| Android 侧未暴露重连信息 | beam-session 已有 SessionHandle::reconnecting()，但 uniffi 导出面还没有对应映射 | 下一批 |
| §3.9 转移表缺 Negotiating → Reconnecting | 协商期传输失败只能进 Error 态；改转移表属于改文档语义，未擅自改 | 待裁决 |

| B9 | 2026-10-31 | 第九批：音频进产品链路 + 桌面端真实统计 + FEC 矩阵复用 + uniffi 暴露重连与汇总 | crates/beam-audio / crates/beam-host / crates/beam-client / crates/beam-net / crates/beam-ffi / apps/desktop / android | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（45 套件 / 453 passed / 0 failed / 9 ignored）；cargo test -p beam-net --features fec exit 0（76 例 / 0 失败）；cargo bench -p beam-net --features fec --bench packetize exit 0（T3 达标、T1/T2 仍超预算，见 N-24）；cargo check -p beam-ffi --features uniffi --all-targets exit 0；桌面端 clippy --all-targets -- -D warnings exit 0；gradlew assembleDebug assembleRelease test --rerun-tasks exit 0（28 测试类 / 184 次 / 0 失败）；apk_no_perf.py 门禁 exit 0（2 个 release APK 的 dex 中无 perf 类） |

- **B9-A 音频进产品链路** — 写入范围：`crates/beam-audio/**`、`crates/beam-host/**`、`crates/beam-client/**`；覆盖条目 DX-02, A-14
- **B9-B FEC 矩阵复用（主会话自办）** — 写入范围：`crates/beam-net/**`；覆盖条目 N-24
- **B9-C uniffi 暴露重连与汇总（主会话自办）** — 写入范围：`crates/beam-ffi/**`、`android/**`；覆盖条目 
- **B9-D 桌面端真实统计（主会话自办）** — 写入范围：`apps/desktop/**`；覆盖条目 EV-02, EV-05, EV-08, CM-13, CM-14

**B9 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| B8 追加项「Android 侧未暴露重连信息」已收口 | uniffi 导出面新增 `Session::reconnecting()` / `Session::summary()`（**查询式**，不是新增 `SessionEvent` 变体 —— 变体会同时打破 beam-session / beam-ffi / 桌面端三处穷尽匹配）；Android 侧 `NativeMappings` 映射 + `Session` 接口 + `StreamViewModel` 按 500 ms 轮询真实重连状态（原先 `SessionEvent.Reconnecting` 变体 Rust 从不发出，断线遮罩是死代码） | 已收口 |
| FEC 预算：T1/T2 仍超 1.8~1.9 倍 | 矩阵重建这个根因已消除，剩下的是标量 GF 乘加本身。再降要动协议口径或换依赖 —— 登记为待决 S14 | 待裁决 |
| 音频真实出声与音画同步仍未验证 | B9-A 把采集→Opus→datagram→解码→WASAPI 输出整条接通，但本机无声卡回环条件，**「声音真的能出来」未验证**；音画同步也未测（两个时钟不同源）。§3.7 的采集→编码 ≤6 ms / 端到端 ≤40 ms / 同步 <30 ms 三个指标仍未测 | 需真机人工验证 |
| Android 端到端运行时行为仍未验证 | 无 Android 真机 ⇒ 真机上的建连、输入回传、重连恢复、音频播放全部未实测 | 需真机人工验证 |

| B10 | 2026-10-31 | 第十批：桌面端客户端真实上屏（独立原生窗口 + D3D11VA 硬解 + flip-model 交换链） | crates/beam-render / crates/beam-decode / crates/beam-client / apps/desktop | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（46 套件 / 453 passed / 0 failed / 9 ignored）；cargo check/clippy -p beam-client --features video exit 0 / 0 warning；cargo test -p beam-client --features video --test video_present -- --ignored exit 0（独立原生窗口 + D3D11 flip-model 交换链 + D3D11VA 硬解上下文三步都真的建得起来）；桌面端 clippy --all-targets -- -D warnings exit 0（默认与 --features client-video 两种状态）；npm --prefix apps/desktop run build exit 0 |

- **B10 客户端上屏链路（主会话自办）** — 写入范围：`crates/beam-render/**`、`crates/beam-decode/**`、`crates/beam-client/**`、`apps/desktop/**`；覆盖条目 ST-07

**B10 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 「画面真的出现在独立窗口里」未做端到端验证 | 本机自检只验证了「窗口 + 交换链 + 硬解上下文」建得起来；真实主机 → 网络 → 解码 → 上屏的完整链路需要 GUI 环境的串流会话，未自动化验证。上屏帧率 / 延迟 / 丢帧指标也未实测 | 需人工验证 |
| FEC 帧的热路径仍未走 DirectByteBuffer（F-07 / P6-06） | 查证：uniffi 0.29 的 Kotlin 后端没有 ByteBuffer 直传；0.32 的 `lower_type_name_for_arg` 只对「Kotlin → Rust」方向的借用 `Bytes` 生效，**回调方向（Rust → Kotlin）仍然映射成 ByteArray**。因此零拷贝回调需要手写 JNI ⇒ 仍卡在待决事项 S4（新增 `jni` 依赖 + 放开 `beam-ffi` 的 `forbid(unsafe_code)`） | 待裁决 |
| §3.10 的 `beam-render` 职责是否包含「创建独立原生串流窗口」 | 本批把窗口放进 `beam-render`（交换链与宿主窗口成对），§3.10 原文只写「D3D11 flip-model 交换链上屏；叠加层由 Tauri 窗口负责」。**语义没变**（客户端仍渲染到独立原生窗口），只是承载代码的 crate 变了。若认为这属于改文档口径，请裁决是否补一句 | 待裁决 |

| B11 | 2026-10-31 | 第十一批：GPU 上的 RGB→NV12 走 compute shader（§3.6 允许的第二条路） | crates/beam-encode | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo clippy -p beam-encode --features ffmpeg --all-targets -- -D warnings exit 0（此前这条本来就不过）；cargo test --workspace exit 0（47 套件 / 455 passed / 0 failed / 9 ignored）；cargo test -p beam-encode --test color_compute --release exit 0（Y 平面自 NV12 纹理读回、与 CPU 参考逐字节一致，偏差 0；色度经影子平面偏差 0；1080p 1.02 ms、4K 2.10 ms，含 GPU 同步）；桌面端 clippy --all-targets -- -D warnings exit 0 |

- **B11 compute shader 颜色转换（主会话自办）** — 写入范围：`crates/beam-encode/**`；覆盖条目 E-01

**B11 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 真实 WGC 抓屏纹理 → 编码器的端到端联调未做 | 本批的验证入口自造 BGRA8 纹理；真实链路是「WGC 抓屏 → compute shader 转 NV12 → NVENC → 客户端解码」，端到端（含 NVENC 出码流后解码回来与源画面逐像素比对）未验证 | 需人工验证 |
| 本机驱动的两个 D3D11 限制（已绕开，但要记住） | ① 带 D3D11_BIND_VIDEO_ENCODER 的纹理**不能参与拷贝**（拷进去/拷出来都是全 0）⇒ 颜色转换必须由 shader **直写** NV12 的平面；② NV12 staging 的平面 1（UV）映射返回 E_INVALIDARG ⇒ 色度的校验改用「影子平面」（同一 shader 写出的普通 R8G8 纹理） | 已记录 |
| VideoProcessor 这条路在本机不可用（96 组变体全灭） | 代码保留 VideoProcessor 优先（其它驱动上它更省事：硬件缩放+色彩转换一步到位），建不出来才回退 compute shader。探针 examples/color_vp_probe.rs 留着，方便换机器时再验一次 | 已记录 |

| B12 | 2026-11-01 | 第十二批：客户端时钟偏移（§5 ClockSync 求偏移） | crates/beam-session | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（47 套件 / 461 passed / 0 failed / 9 ignored，其中 beam-session 新增 6 个：偏移估计 4 个 + 5 s 节拍 + 应答不回声）；桌面端 clippy --all-targets -- -D warnings exit 0 |

- **B12 时钟同步（主会话自办）** — 写入范围：`crates/beam-session/**`；覆盖条目 P6-03, X13-08

**B12 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| E2E 延迟测量（§3.13）还没用上这个偏移 | 偏移已经能查了，但客户端渲染侧还没记录「上屏时刻（客户端时钟）」，因此「抓屏时刻 + 偏移 → 上屏时刻」的差值（真实 E2E）还没算出来。X13-08 因此记「进行中」 | 下一批 |
| 时钟口径的已知局限 | 两端都用各自进程的**单调**时钟（零点不同），偏移正是用来换算这个差异；但单调时钟**不与墙钟对齐**，因此偏移量在两台机器上不可直接比较——这不影响 E2E 测量（同一对端会话内偏移是稳定的） | 已记录 |

| B13 | 2026-11-01 | 第十三轮：§3.13 的 E2E 延迟测量接线（时间戳路径） | crates/beam-session / crates/beam-client | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo clippy -p beam-client --features perf-stats --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（48 套件 / 461 passed / 0 failed / 9 ignored）；cargo test -p beam-client --features perf-stats --test e2e_measure exit 0（偏移 100 ms 时 E2E 测得约 200 ms；无偏移时不记样本）；桌面端 clippy --all-targets -- -D warnings exit 0 |

- **B13 E2E 测量（主会话自办）** — 写入范围：`crates/beam-session/**`、`crates/beam-client/**`；覆盖条目 X13-08, X13-09

**B13 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| E2E 交叉校验还差「分段路径」 | 采集器是每进程的：主机侧（抓屏/颜色转换/编码）与客户端侧（解码/上屏）的阶段直方图分处两个进程，要把主机侧的阶段耗时传过来才能求和做交叉校验 | 下一批 |
| E2E 的真实数字仍未测出 | 测量链路已通，但没有真实串流会话，E2E 的实际数值（目标 T0 ≤30 ms / T1 ≤27 ms）未实测 | 需人工验证 |

| B14 | 2026-11-01 | 第十四批：mDNS 服务类型修复 + 真机回环首次整链跑通 | crates/beam-discovery / crates/beam-host（examples） | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（48 套件 / 462 passed / 0 failed / 9 ignored）；cargo run -p beam-host --example host_loopback --features platform --release exit 0：**本机真机整链首次跑通** —— mDNS 广播与发现 ✓、信任闸门拒绝未信任对端并记录指纹 ✓、人工确认后写库 ✓、重连协商到 Streaming ✓、**客户端收到 241 帧真实画面**（WGC 抓屏 → compute shader 颜色转换 → NVENC → QUIC datagram） |

- **B14 发现层修复与真机回环（主会话自办）** — 写入范围：`crates/beam-discovery/**`、`crates/beam-host/**`；覆盖条目 D-01, D-06

**B14 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 客户端的「解码 + 上屏」这一段仍未在真机链路上验证 | 回环里的客户端是**最小实现**（只数帧，不解码不上屏）；真实客户端的 D3D11VA 硬解 + D3D11 flip-model 上屏（B10 的原生窗口路径）还没在这条链路上跑过 | 下一批 |
| 回环示例的三个顺序/时限问题（都在示例里，不是产品代码） | ① 先 connect 后 accept ⇒ quinn 没人推进握手，客户端只能超时，主机也看不到客户端证书；② 最小客户端不发 Feedback ⇒ 主机 3 s 停滞判定触发；③ 客户端时限短于主机 ⇒ 客户端先断开，主机会话按正常断开语义进 Error 态，示例的判据却要求主机仍是 Streaming | 已修 |
| mDNS 在真实局域网里的可达性未验证 | 本机回环只能证明「广播能被同一台机器浏览到」；跨机器的组播可达性、路由器是否放行 `_beam._udp` 的组播，仍需两台机器实测 | 需人工验证 |

| B15 | 2026-11-01 | 第十五批：裁决 14 条待决策（S1–S14）并落地 | crates/beam-net / crates/beam-discovery / docs（两份设计文档） | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0；cargo bench -p beam-net --features fec --bench packetize exit 0：**§3.2 的 FEC 预算全部达标**（T1 0.670 / T2 0.376 / T3 0.152 ms）；两份设计文档按裁决改写（CRLF + UTF-8 无 BOM 保持不变） |

- **B15 决策落地（主会话自办）** — 写入范围：`crates/beam-net/**`、`docs/**`；覆盖条目 N-24, N-29

**B15 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| S5（错误码段重划）与 S11（PIN 校验）已裁但未实施 | S5 要同步改桌面 Rust 端 + 前端文案表 + §3.11/§3.14 清单；S11 要改 connect 的 command 签名 + 前端。都排在下一批 | 下一批 |
| S4（DirectByteBuffer 零拷贝）按当前预算判定为「不必做」 | 拷贝量级是**估算**（1080p@20 Mbps ≈ 2.5 MB/帧 ≈ 0.2 ms/帧），未实测；若将来要把 §6 的零拷贝写成已达成，需要先实测这个量级 | 需人工验证 |
| S1 的 `plat` 字段实现 | §3.4 已写明字段语义，但 beam-discovery 的 TXT 与信任库写入路径还没接上该字段（发现到的主机仍显示「未知平台」） | 下一批 |

| B16 | 2026-11-01 | 第十六批：S1（plat 贯通）/ S5（错误码段重划）/ S11（connect 补 PIN）落地 | crates/beam-discovery / crates/beam-ffi / crates/beam-log / apps/desktop（Rust + 前端）/ android（Kotlin）/ docs（设计文档） | 已完成 | cargo fmt --all --check exit 0；cargo check --workspace --all-targets exit 0 / 0 warning；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（48 套件 / 466 passed / 0 failed / 9 ignored）；桌面端 cargo test exit 0（14 套件 / 14 passed，含 6 条新增的 connect PIN 用例）；桌面端 clippy（默认 / host-platform-wgc / perf-stats 三种组合）全部 exit 0 / 0 warning；npm --prefix apps/desktop run build exit 0（typecheck + vite build）；gradlew test --rerun-tasks exit 0（Android 28 测试类 / 186 用例 / 0 失败，含 2 条新增的 plat 贯通用例）；python tools/ci/check-design-tokens/check_design_tokens.py exit 0（54 项全过）；两份设计文档按裁决改写（CRLF + UTF-8 无 BOM 保持不变） |

- **B16（主会话自办）** — 写入范围：`crates/beam-discovery/**`、`crates/beam-ffi/**`、`crates/beam-log/**`、`apps/desktop/**`、`android/**`、`docs/**`；覆盖条目 S-1, S-5, S-11

**B16 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| SPAKE2 的 PIN 密码学校验仍未接线 | B16 只做了格式校验与尝试预算。真正判定「PIN 对不对」要靠主机侧的 SPAKE2 交换（§3.3），而桌面端主机还没把 PIN 配进 `PairingConfig`，且主机侧信任闸门在 TLS 层就拒绝未信任对端 —— 客户端根本到不了控制通道。补齐需要改动主机侧信任闸门的握手顺序（§3.3 的待办） | 需后续批次 |
| Android 端的 `plat` 只到发现结果，未进信任库 | Android 的 `core-data` 信任库存的是 `Peer.platform`，配对成功后由 `PairingViewModel` 写入；但 Android 侧的配对走扫码（§3.3 载荷不带平台）⇒ 落库的仍是「未知平台」。要补齐需要让 Android 客户端把发现阶段拿到的 `plat` 关联到信任记录 | 需后续批次 |
| E13xxx/E14xxx/E15xxx 仍未写进设计文档 | B16 把 E16xxx（log）写进了 §3.15，但 perf / ffi / gamestream 三段的编号归属仍只在代码里。它们与 E12xxx 的归属一样存在撞号风险，建议下一批一并写进 §3.13 / §3.12 | 需后续批次 |

| B17 | 2026-11-01 | 第十七批：客户端「解码 + 上屏」首次真机验证，定位并暴露了一个假成功 | crates/beam-render / crates/beam-client（examples）/ apps/desktop（前端文案表）/ docs（设计文档 §3.10） | 已完成 | cargo fmt --all --check exit 0；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo clippy -p beam-render --features d3d11 --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（48 套件 / 466 passed / 0 failed / 9 ignored）；cargo test -p beam-render exit 0；npm --prefix apps/desktop run build exit 0（typecheck + vite build）；**真机实测**（RTX 3070 / 驱动 596.49 / 1920×1080 交互式桌面，FFMPEG_DIR + LIBCLANG_PATH 已设）：`cargo run -p beam-client --example present_smoke --features video --release` —— 如实报 FAIL 并给出完整对照实验表（420Opaque / NV12 三种高度建链全灭；BGRA 三种高度均「真搬 0 次 / 降级 1 次，源格式 0x67」）；`cargo run -p beam-decode --example d3d11va_decode_smoke --features ffmpeg --release` exit 0（硬解本身正常：9 帧 1920×1080）；设计文档 §3.10 已按实测结论改写（CRLF + UTF-8 无 BOM 保持不变） |

- **B17（主会话自办；一个 worker 因 Token Plan 配额耗尽未能开工）** — 写入范围：`crates/beam-render/**`、`crates/beam-client/examples/**`、`crates/beam-client/Cargo.toml`、`apps/desktop/src/i18n/**`、`docs/**`；覆盖条目 ST-07

**B17 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 4:2:0 → BGRA 的转换路径未实现（画面仍然上不了屏） | 本批用对照实验证明了**零拷贝不可行**（flip-model 交换链不接受 4:2:0 视频格式；VideoProcessor 在本机建不出输入视图），因此需要 PSV + 像素着色器把 DXGI_FORMAT_420Opaque 双平面纹理转成 BGRA 再写入后备缓冲。两个待查的技术点：① D3D11 的 shader 不能直接采样多平面纹理，必须走 Pixel Shader View，而 PSV 的 plane slice 语义在 420Opaque 下需要实测；② 纹理高 1152 但可见高 1080，多出的 72 行是 padding，采样必须按可见高度截断 | 下一批 |
| V-13（主机侧丢帧统计接入 host 编排）仍未做 | 已定位：`crates/beam-session/src/host.rs` 的 `SessionCounters::dropped_frames` 恒为 0；而抓屏单槽（`crates/beam-capture/src/slot.rs`）覆盖旧帧时**根本没有计数**，客户端侧渲染单槽的丢帧（`crates/beam-decode/src/slot.rs` 的 `FrameSlot::dropped()`）也没进会话汇总。补齐需要给 `CaptureSource` / `FramePipeline` 加取丢帧的接口 | 下一批 |
| 整链回环（抓屏→NVENC→QUIC→硬解→上屏）尚未跑通 | `crates/beam-host/examples/host_loopback.rs` 的客户端只数帧、不解码不上屏。整链示例要等上屏转换路径落地才能验证「画面真的出现在窗口里」 | 下一批 |

| B18 | 2026-11-01 | 第十八批：画面真的上屏了（VideoProcessor 转换路径）+ V-13 丢帧统计贯通 | crates/beam-render / crates/beam-client / crates/beam-capture / crates/beam-session / crates/beam-host / docs（设计文档 §3.10） | 已完成 | cargo fmt --all --check exit 0；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo clippy -p beam-render --features d3d11 --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（48 套件 / 470 passed / 0 failed / 9 ignored，含新增的 3 条丢帧统计用例）；cargo test -p beam-session exit 0（53 + 6 + 2）；cargo test -p beam-capture exit 0（新增 1 条单槽丢帧计数用例）；npm --prefix apps/desktop run build exit 0；**真机实测**（RTX 3070 / 驱动 596.49 / 1920×1080 交互式桌面）：`cargo run -p beam-client --example vp_probe --features video --release` exit 0 —— **输入视图用 '420O' 建得起来**（枚举器建成；DXGI 枚举值与 'W8B8' 全部失败）；`cargo run -p beam-client --example present_smoke --features video --release` exit 0 —— RESULT: PASS，9 帧全部经转换后搬进交换链（真搬 9 / 转换 9 / 降级 0）；`cargo run -p beam-host --example host_loopback --features platform --release` exit 0 —— 真机整链仍然通过（客户端收到 242 帧）；设计文档 §3.10 已按实测结论改写（CRLF + UTF-8 无 BOM 保持不变） |

- **B18（主会话自办；worker 仍因 Token Plan 配额耗尽不可用）** — 写入范围：`crates/beam-render/**`、`crates/beam-client/**`、`crates/beam-capture/**`、`crates/beam-session/**`、`crates/beam-host/**`、`docs/**`；覆盖条目 ST-07, V-13

**B18 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 整链回环（抓屏→NVENC→QUIC→硬解→上屏）尚未跑通 | `crates/beam-host/examples/host_loopback.rs` 的客户端只数帧、不解码不上屏。上屏转换路径已落地，现在可以把它接进回环示例，验证「画面真的出现在窗口里」 | 下一批 |
| 转换路径的性能还没实测 | VideoProcessor 转换 + CopyResource 的单帧耗时未测（B18 只验证了「能上屏」）。§3.10 的渲染预算（按 vblank 上屏）需要对照实测 | 下一批 |
| E13xxx/E14xxx/E15xxx 仍未写进设计文档 | 与 B16 遗留一致：perf / ffi / gamestream 三段的编号归属只在代码里 | 需后续批次 |

| B19 | 2026-11-04 | 第十九批：整链回环真机跑通（抓屏→NVENC→QUIC→硬解→画面上屏） | crates/beam-host（示例 + 可选依赖 + perf_span 占位符统一）/ docs（设计文档 §3.10） | 已完成 | cargo fmt --all --check exit 0；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（48 套件 / 470 passed / 0 failed / 9 ignored）；cargo clippy -p beam-host --features platform,client-video --all-targets -- -D warnings exit 0 / 0 warning；cargo clippy -p beam-capture --features wgc --all-targets -- -D warnings exit 0；cargo clippy -p beam-render --features d3d11 --all-targets -- -D warnings exit 0；cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings exit 0；npm --prefix apps/desktop run build exit 0；**真机实测**（RTX 3070 / 驱动 596.49 / 1920×1080 交互式桌面）：`cargo run -p beam-host --example client_loopback --features platform,client-video --release` exit 0 —— **抓屏→NVENC→QUIC→硬解→上屏 整链通过**：收到 345 帧，「真的搬进交换链」345 帧（其中经 VideoProcessor 转换 345）、降级 0；`cargo run -p beam-host --example host_loopback --features platform --release` exit 0 —— 既有回环仍然通过（真机整链，未受本次改动影响）；设计文档 §3.10 已按实测结论改写（CRLF + UTF-8 无 BOM 保持不变） |

- **B19（主会话自办）** — 写入范围：`crates/beam-host/**`、`crates/beam-capture/src/perf.rs`、`crates/beam-decode/src/perf.rs`、`crates/beam-encode/src/perf.rs`、`docs/**`；覆盖条目 ST-07

**B19 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 转换路径的性能还没实测 | VideoProcessor 转换 + CopyResource 的单帧耗时未测（B18/B19 只验证了「能上屏」）。§3.10 的渲染预算（按 vblank 上屏）需要对照实测 | 需后续批次 |
| 整链回环还没有覆盖音频与输入回传 | `client_loopback` 验证的是画面链路（§3.10）。音频（§3.7 WASAPI loopback + Opus）与输入回传（§3.8 SendInput / ViGEm 手柄）在这条链上还没验证过 | 需后续批次 |
| E13xxx/E14xxx/E15xxx 仍未写进设计文档 | 与 B16 遗留一致：perf / ffi / gamestream 三段的编号归属只在代码里 | 需后续批次 |

| B20 | 2026-11-04 | 第二十批：音频进整链（真机）+ 上屏性能实测 + 修掉主机音频只能跑 1/3 速率的缺陷 | crates/beam-audio（receive 模块 + 速率对照入口）/ crates/beam-net（DemuxStream 分流读点）/ crates/beam-client（分流改用 beam-net）/ crates/beam-host（音频接线 + stop_audio + 示例）/ docs（设计文档 §3.2 / §3.7 / §3.10） | 已完成 | cargo fmt --all --check exit 0；cargo clippy --workspace --all-targets -- -D warnings exit 0 / 0 warning；cargo test --workspace exit 0（48 套件 / 475 passed / 0 failed / 9 ignored）；cargo clippy -p beam-host --features platform,client-video --all-targets -- -D warnings exit 0；cargo clippy -p beam-host --features platform,client-video,perf-stats --all-targets -- -D warnings exit 0；cargo clippy -p beam-capture --features wgc --all-targets -- -D warnings exit 0；cargo clippy -p beam-render --features d3d11 --all-targets -- -D warnings exit 0；cargo clippy -p beam-audio --features opus --all-targets -- -D warnings exit 0；cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings exit 0；npm --prefix apps/desktop run build exit 0；设计令牌检查 54 项全过；清单引用 909 条全部有效（verify_refs 0 无效）；**真机实测**（RTX 3070 / 驱动 596.49 / 1920×1080 交互式桌面）：`cargo run -p beam-host --example client_loopback --features platform,client-video,perf-stats --release` exit 0 —— **画面 285 帧全部经 VideoProcessor 转换后搬进交换链、降级 0**；**音频 800 帧走完整链**（采集发出 800 / 客户端解码 800 / 回放 800，错误 0），解码 PCM 的 RMS = 0.084、峰值 = 0.148（非静音 ⇒ 采到的是真声音）；采集队列**零积压**（修复前刷「积压，丢弃最旧的一帧」且只有 65 帧/秒）；性能：解码均值 0.348 ms vs 预算 8 ms ⇒ 达标，上屏均值 0.106 ms vs 半帧预算 8.33 ms ⇒ 达标；`cargo run -p beam-audio --example audio_rate_probe --features opus --release` exit 0 —— 采集 199.5 / +Opus 编码 199.6 / +建数据报 199.7 帧/秒 ⇒ 处理能力本身够用，瓶颈在循环结构 |

- **B20（主会话自办）** — 写入范围：`crates/beam-audio/**`、`crates/beam-net/src/demux.rs`、`crates/beam-client/src/{audio,transport}.rs`、`crates/beam-host/**`、`docs/**`；覆盖条目 ST-07, V-10, V-11, DX-02, A-01, A-04

**B20 追加项（审计中顺带发现，不在原 551 条内）**

| 项 | 说明 | 归属 |
|---|---|---|
| 客户端 PIN 的 SPAKE2 密码学校验仍未接线 | 主机侧未把 PIN 配进 PairingConfig，且主机信任闸门在 TLS 层就拒绝未信任对端 ⇒ 客户端到不了控制通道（B16 遗留） | 需后续批次 |
| Android 端的 plat 只到发现结果，未进信任库 | Android 配对走扫码，§3.3 的二维码载荷不带平台标识 ⇒ TrustedPeer.platform 仍是未知平台 | 需后续批次 |
| 上屏『等 vblank 的等待时长』仍测不了 | flip-model 交换链的 GetFrameStatistics 不可用（B18 已记）⇒ V-11 保持已阻塞；本批只测到了上屏**成本** | 需后续批次 |
| 音画同步偏差（§3.15 <30 ms）仍无测量 | 需要跨设备时钟同步（四时间戳），本机同机回环量不出真实偏差（X8-16） | 需后续批次 |

### 已裁决事项

| # | 议题 | 裁决 | 裁决日期 | 说明 | 解锁范围 |
|---|---|---|---|---|---|
| D1 | 是否放开 #![forbid(unsafe_code)] | **只放开平台层** | 2026-10-29 | 平台 crate 内改 #![deny(unsafe_code)] 并在确实需要的模块/函数局部 #[allow(unsafe_code)]，其余 12 个 crate 保持 forbid | §3.5, §3.6, §3.7, §3.8, §3.10 |
| D2 | FEC 在 MVP 阶段关还是开 | **按文档：MVP 关闭** | 2026-10-29 | 给 beam-net 加 fec feature（默认关），v1 再开 | §3.2 |
| D3 | shard_idx 语义以文档还是实现为准 | **按文档：改为组内分片号** | 2026-10-29 | 需同步改 net 层分包/重组与测试，并保证接收端在丢包/乱序/重复下仍能无歧义确定组边界 | §5 |
| D4 | windows crate 版本分叉（0.61 / 0.62） | **保留现状：beam-capture 内以别名引入 windows 0.61.3 做 COM 指针桥接，对外 Texture 仍用 windows 0.62** | 2026-10-29 | 文档 §2 只写「依赖 windows-capture、windows crate」，§7 未规定版本 → 属实现细节而非文档语义，故按「只改实现方式不改语义」裁决为保留现状。理由：①windows-capture 2.x 需 rustc >= 1.88，与仓库 MSRV 1.86 冲突，升不动；②D3D11 的 IID/vtable 由操作系统定义，两版 ABI 相同，桥接安全；③对外的 Texture 必须与 beam-encode 的 D3D11 纹理同型才能零拷贝。副作用：依赖树里同时存在 windows 0.61/0.62 两套 windows_result。**未改文档语义。** |  |
| D5 | 跨 crate 的 D3D11 设备契约由谁保证 | **设备由串流编排层 beam-host（L3）统一创建，并注入 capture / encode / decode / render；平台 crate 之间不横向依赖** | 2026-10-29 | 文档 §4 规定 L1 之间不可横向依赖（唯一例外 net→pair），§6 规定「D3D11 纹理用引用计数 COM 指针，capture→encode 同一设备上零拷贝」。用平台类型 ID3D11Device 作契约参数、由 L3 注入，是唯一同时满足这两条的形状。B3-D 已落地 D3D11VaDecoder::open_with_device(cfg, &ID3D11Device)；B3-A 的 D3D11Renderer::device() 已做运行期设备校验（不匹配则该帧清屏并告警，不静默拷错）。**未改文档语义。** |  |
| D6 | NVENC 一帧流水线延迟在 host 侧如何处理 | **host 侧显式容忍 E6005（视作「本轮暂无输出」），且不在 host 侧再缓存一帧** | 2026-10-29 | 文档 §3.6 未规定 NVENC 延迟；§4.1 时序允许首帧晚一点到（客户端会等到 IDR）。在 host 侧缓存一帧会让主机比真实画面多持有一帧，把延迟做大，与 §4.1「上屏等待含 vsync 整帧量化、目标须留 1 帧余量」相悖。E6005 是段内扩展码，不进错误槽；真错误继续冒泡，不静默吞掉。**未改文档语义。** 仍缺：桌面端主机侧尚未消费该错误通道（start_hosting 未起 QUIC 监听）。 |  |
| S1 | 设备平台字段（platform）缺失 | **mDNS TXT 追加 `plat` 字段（§3.4），不改 §5 协议** | 2026-11-01 | 信任库的平台字段只需要知道「对端是 Windows 还是 Android」，这个信息在**发现**阶段就有，不必等控制面握手，也不必新增 §5 控制消息。TXT 是**追加**语义：对端不带该字段时按「未知平台」处理，不影响发现与配对。§3.4 已写明 `plat` 的语义与向后兼容性 | §3.4, §5 |
| S2 | 帧头缺少独立的 pts 字段 | **不改帧头；把「`pts_us = capture_ts_us`」写成显式约定** | 2026-11-01 | 低延迟串流刻意不做重排序缓冲，pts 与抓屏时刻同源才是想要的语义（多一个独立 pts 只会诱使人去做排序缓冲）。16 字节帧头已占满，加字段要重排位宽 ⇒ 协议不兼容变更，收益为负。§3.2 / §5 已写明该约定 | §5 |
| S3 | §5 自相矛盾：k ≤ 200 与「单帧超过 255 片拆多组」 | **统一为：单组总分片 `k+m ≤ 255`；`k` 是实现侧的分组粒度（实现取 64）** | 2026-11-01 | 两条原规则互斥。统一口径：「255」是**单组总分片数**上限（帧头 `data_shards`/`fec_shards` 各 8 位，但组内总分片受限）；「k」只是分组粒度，由实现按 §3.2 的性能预算来选。这样既不改帧头布局，也消除了矛盾。§5 已改写；「k = 64」由 S14 的实测支撑 | §5, §3.2 |
| S4 | JNI DirectByteBuffer 零拷贝 | **暂不引入 `jni` 依赖；保留 uniffi 的 `Vec<u8>` 拷贝路径** | 2026-11-01 | 已核实：uniffi 0.29 的 Kotlin 后端没有 ByteBuffer 直传；0.32 只对「Kotlin → Rust」方向的借用 `Bytes` 生效，**回调方向（Rust → Kotlin）仍然映射成 ByteArray**。要零拷贝只能手写 JNI ⇒ 新增 `jni` 依赖 + 放开 beam-ffi 的 `forbid(unsafe_code)`，成本与风险都不小。而拷贝量级（1080p@20 Mbps ≈ 2.5 MB/帧 ≈ 0.2 ms/帧，**这是估算不是实测**）远小于 §3.6 的 7 ms 预算，不是瓶颈；真要优化应先降码率/帧大小。§6 的「零拷贝」要求记为**已知未达成**，不冒充已实现 | §3.12, §6 |
| S5 | 错误码段撞车：外壳与 crates 同码不同义 | **E11xxx 留给配置层（§3.14）；桌面端外壳的错误改用 E12xxx（外壳段）** | 2026-11-01 | E11xxx 同时被 beam-config（解析/校验/存储不可用）与桌面端外壳（设置不合法/读写失败/perf 仅 Debug）当配置类用，E11003 一个码两种含义（perf 不可用 vs 配置存储不可用），前端按 code 唯一只能写合并文案 ⇒ 报错不可定位。E12xxx 已是外壳段（E12001 未知命令），把外壳的错误挪进去语义一致，且**不新增错误段**。实现排在下一批（要同步改桌面 Rust 端 + 前端文案表 + §3.11/§3.14 清单） | §3.11, §3.14 |
| S6 | Android 侧键码无法编码成 USB HID | **不扩协议；能映射的映射，映射不上的丢弃并计入 E8004** | 2026-11-01 | BrowserBack / BrowserHome / ContextMenu 这类键在 HID Keyboard page 根本没有 usage —— 它们是**浏览器/系统键**而不是游戏输入，串流协议不需要它们（Android 端的返回键由 App 自己处理，不作为游戏键上行）。扩展协议键空间要改 §5 消息语义，收益为负。既有 E8004（同段内扩展）已能如实报告「这个键采不上来」 | §3.8 |
| S7 | 滚轮的物理单位未定义 | **Q16.16 定点数，**整数部分 = 滚轮刻度**；Windows 注入侧再乘 `WHEEL_DELTA=120`** | 2026-11-01 | 与主流实时串流实现一致（一格 = 120 是 Win32 的 `WHEEL_DELTA`）。Q16.16 保留小数部分给高精度滚轮/触控板。§3.8 已写明 | §3.8 |
| S8 | E8004 是 E8xxx 段内扩展码，文档未列 | **补进 §3.8 的错误码清单，标注为 E8xxx 段内扩展** | 2026-11-01 | 与既有 8001/8002 同段，没有新增错误段、没有改既有语义，只是把清单补全。§3.8 已写明 | §3.8 |
| S9 | §16 内部张力：「间距 4 的倍数」与「卡片内边距 14」 | **「4 的倍数」只约束**布局间距**；组件内边距按逐字表取值** | 2026-11-01 | 逐字表里卡片内边距 14 与「间距 4 的倍数」本来就不可能同时成立，而设计规范门禁也只校验逐字表。限定措辞即可让两者自洽，实现不动。产品方案 §16 已改写 | §16 |
| S10 | §16 的「手机外框 30」在桌面端没有使用点 | **令牌保留（两端共用同一刻度），桌面端不引用** | 2026-11-01 | 手机外框是 Android 端手机模型的圆角，桌面端没有这个形状，强行引用才是错。门禁把它排除在「必须被用到」清单外。产品方案 §16 已标注「仅 Android 端手机模型使用」 | §16 |
| S11 | 桌面端 PIN 没有端到端校验 | **connect 补 PIN 参数 + Rust 侧格式校验与尝试预算；SPAKE2 PIN 挑战留待 §3.3** | 2026-11-01 | 现状是后端 `connect(host_id)` 根本没有 PIN 参数 ⇒ 填对填错都能连上，前端的 PIN 输入框是装饰。诚实做法要么补校验、要么明说它现在不参与安全。**先做前者**（格式校验 + 尝试预算 + 失败计数），并把「主机侧的 PIN 挑战仍未实现」写进文档，不让 UI 假装在做密码学。实现排在下一批（要改 command 签名 + 前端） | §3.3, §3.11 |
| S12 | 通用 UI 组件放在 :core-data 的 ui 包 | **通用 UI 组件与设计令牌同置于 :core-data/ui，不改 §3.12 的模块表** | 2026-11-01 | §3.12 固定了 12 个模块且没有 :core-ui；重划模块要改文档 + 搬一堆代码，而依赖方向（:feature-* → :core-data）本来就不变。§3.12 已补一句说明 | §3.12 |
| S13 | 音频帧的 FEC 冗余片无法与视频区分 | **音频帧占用**独立帧号号段**且**不参与 FEC 分包**（`fec_shards=0`）** | 2026-11-01 | B9-A 已按这个口径实现（`frame_id` 最高位作号段标志）。音频是低码率单片，FEC 对它收益极低而复杂度高；这样 `type=2` 仍只表示「视频帧的 FEC 冗余片」，接收端不会混淆。§5 已写明 | §5, §3.7 |
| S14 | §3.2 的 FEC 预算：T1/T2 仍超标 1.8~1.9 倍 | **把分组粒度 `k` 从 200 降到 64（依赖 S3 的口径统一）** | 2026-11-01 | RS 编码代价 ∝ k，降到 64 后实测：**T1 0.670 ms（预算 0.8）、T2 0.376 ms（预算 0.4）、T3 0.152 ms（预算 0.3）——全部达标**；恢复路径同样全部达标。代价是每帧组数变多（T1 从 2 组到 5 组），组号 5 位上限 32 仍够用。不改帧头布局、不改协议语义。simd-accel 与换编码两条路**不采纳**（msvc 下上游不编译 SIMD 代码；换编码要换依赖） | §3.2, §5 |

### 本轮修复与提交记录

| 日期 | 事项 | 说明 | 结果 |
|---|---|---|---|
| 2026-10-29 | 修复 beam-host 三处断点 | B4-B 在接 E6005 改造时被 Token 配额掐断，留下：(1) adapters.rs:265/272 测试模块用了不存在的 CaptureCapturedFrame（应为 beam_capture::CapturedFrame），(2) runtime.rs:323 HostSessionHandle 初始化缺新增的 errors: PipelineErrors 字段，(3) runtime.rs:377 缺 state() 方法。已逐处修复并补 import。 | cargo check -p beam-host --all-targets exit 0 |
| 2026-10-29 | cargo fmt --all 归位 | B4-B 遗留的 beam-host/src/{adapters,runtime}.rs 格式偏差导致 fmt exit 1；执行 cargo fmt --all 只重排了这 2 个文件。 | root 与 desktop 两处 cargo fmt --check 均 exit 0 |
| 2026-10-29 | 裁决 D4 / D5 / D6 | 三项均按「只改实现方式、不改文档语义」裁决，理由与影响已记入「已裁决事项」表，并在 beam-capture / beam-host / beam-encode 的模块文档里各加一段「决策备注」。 | 文档语义未变 |
| 2026-10-29 | 拆分提交 86 项悬空改动 | ① d1393e0 平台层（capture/render/decode/encode + Cargo.lock + 技术设计文档）；② 95d5378 协议与网络（net/ffi）；③ 9e7632b 编排与主机（session/client/host）；④ f9c20ef Android 与桌面端。E6005 半成品的产生侧在 ①、消费侧在 ③，按 hunk 无法拆成独立提交（会留下编译不过的中间提交），故在 ③ 的提交信息里明确标注了完成度与未接通点。 | git status 干净，工作区无残留 |
| 2026-10-30 |  | B5-A/B5-B/B5-C/B5-D 四个临时 worker 全部在收尾阶段命中 Token Plan 上限（2056），代码已落盘但验收未跑完。由主会话接手：复核全部落盘代码、补完 B5-A 未做的桌面端主机模式、跑完整验收并补 clippy 清零 | cargo fmt/check/clippy(-D warnings)/test 全绿 |
| 2026-10-30 |  | 根工作区 15 条（beam-perf 新增 8 + 预存 7：derivable_impls / needless_update / manual_range_patt / unnecessary_cast / repeat_n）+ 桌面端 9 条（duplicated_attributes / manual_range_contains / needless_as_bytes / field_reassign_with_default / clone_on_copy / redundant_closure）。注意 CI 的 clippy 带 -D warnings，这些告警会直接让流水线变红 | 根工作区与桌面端（默认 / --no-default-features / --features host-platform 三种组合）clippy 均 0 warning |
| 2026-10-30 |  | 关闭态展开成 () 会被 clippy 的 unused_unit / unneeded_unit_expression（该 lint 在 clippy 版本之间改过名）盯上，跨版本必炸。改为展开成零大小占位类型 NoopSpan：语义不变（仍然零开销），但任何 clippy 版本都不告警 | cargo clippy --workspace --all-targets -- -D warnings exit 0 |
| 2026-10-30 |  | beam-host/src/net.rs 原先自带一份 FRAME_LEN_PREFIX / MAX_FRAME_LEN / FrameDecoder / encode_frame，与 beam-client 各存一份（改一边忘另一边就线上不通）。改为调用 beam_net::framing（beam-net 是 L2，host/client 都依赖它，不构成 L3 横向依赖）；对外错误码保持 E9005 不变，收敛的是实现不是语义 | beam-host / beam-net / beam-client 测试全绿；新增「主机侧必须与共享实现产出相同字节」断言 |
| 2026-10-30 |  | part-6（Tauri 桌面端）复用了 part-4 的 S-xx / T-xx 前缀，part-4 的 X-xx 又与 part-3 重名，台账按 ID 查状态会串台（38 个 ID 冲突）。重命名：part-6 的 T-01..T-24 → CM-01..CM-24、S-01..S-05 → TK-01..TK-05；part-4 的 X-01..X-10 → DX-01..DX-10。判定列与说明列未动 | 616 行 / 578 个唯一 ID，跨分片重名 0；verify_refs 0 无效引用 |
| 2026-10-30 |  | android/app/build.gradle.kts 新增 channel 维度与 store / internal 两个 productFlavors；同步 CI 与门禁脚本路径（产物路径是 apk/<flavor>/<buildType>/）。门禁脚本 --apk 改为可给多个且 glob 展开成全部匹配 —— 旧版只取第一个匹配，单 flavor 时看不出问题，两个 flavor 之后会静默漏检一个 | 6 个变体 APK 构建成功；release 门禁（store+internal）exit 0；debug 负样本自检 exit 0；门禁单测 17 例全过 |
| 2026-10-30 |  | cargo build --release -p beam-perf --features perf-stats 产出的 rlib 会永久留在 target/release/deps（cargo 不清理不同 feature 组合的旧产物），之后扫描默认产物就会误报。脚本改按 cargo 的构建元数据区分：每个产物的 <target>/.fingerprint/<包名>-<hash>/lib-<库名>.json 记着它实际启用的 feature 集合，--expect clean 跳过 perf-stats 开启构建的产物，--expect perf 不跳过；读不到元数据时不跳过（宁可误报不漏报）。踩到的两个坑：产物文件用库名（下划线）而 fingerprint 目录用包名（连字符）；cargo 把 features 写成「JSON 数组的字符串」 | rust 门禁 clean 模式 exit 0（跳过 2 个 perf-on 旧产物）、perf 模式 exit 0（仍检出 2 处）；门禁单测 19 例全过 |
| 2026-10-30 |  | B6-A 修了一个先前就存在的编译错误：beam_session::SessionEvent::FramesDropped（B5-B 新增的 E2004 变体）在 beam-ffi 的 uniffi 导出面里从未被映射，--features uniffi 原本就编译不过。按「变体只能加末尾」在 uniffi 枚举末尾追加 FramesDropped{count,code} | cargo check -p beam-ffi --features uniffi exit 0；cargo ndk 交叉编译 + uniffi-bindgen 生成 Kotlin 绑定 exit 0 |
| 2026-10-30 |  | B6-B 初版按任务书的「相对移动」实现，但 proto 的字段文档写的是「移动到归一化坐标（Q16.16）」、模块头写「屏幕宽/高各映射到 0..=65535」—— 那是 Win32 MOUSEEVENTF_ABSOLUTE 的口径，相对移动在协议层没有表达力（客户端只知道「移动到哪」）。已改为绝对移动 | beam-input 20 例单测全过；clippy -D warnings exit 0 |
| 2026-10-30 |  | crates/beam-client/src/runtime.rs:120 的文档注释写「解码会返回 E10005」，而 beam-decode 实际定义的是 E10004（BACKEND_DISABLED）。已改正 | cargo check --workspace exit 0 |
| 2026-10-30 |  | B5-B 在 Rust 侧加了 SessionEvent::FramesDropped，B6-A 把它加进 uniffi 导出面，但 Android 侧还停在「when 兜底 null」—— sealed interface 的穷尽 when 加变体后必须同步补映射与消费分支，否则丢帧到不了 UI。已补 SessionEvent.FramesDropped + NativeMappings 映射 + StreamViewModel 消费（不改相位，只累加统计）+ 单测 | gradlew assembleDebug assembleRelease test exit 0（22 套件 / 158 次 / 0 失败） |
| 2026-10-30 |  | ffmpeg 的原生 opus 编码器 capabilities 带 exp（experimental），avcodec_open2 直接拒绝（-733130664 = AVERROR_EXPERIMENTAL），而且只支持 fltp 采样格式；libopus 变体支持 s16 且非 experimental。§7 要求 LGPL 构建 —— libopus 是 BSD 许可，LGPL 构建带 --enable-libopus 允许，本仓库的 LGPL 构建确实带了 | 5 ms 帧（240 采样 @ 48 kHz）编解码往返互相关 > 0.9（真机实测） |
| 2026-10-30 |  | 编码器有一帧流水线延迟（首次取包返回 EAGAIN → 返回空包，调用方跳过）；解码器首包是 pre-skip 静音。测试因此改成「连送连解多帧 + 互相关找最佳对齐」，而不是逐点比对或固定相位比对（后者会因相位差拿到假阴性） | 单测 6 例全过 |
| 2026-10-30 |  | 正确约定是先 AvSetMmThreadCharacteristicsW("Games", &index) 拿到 task index，再用 AvSetMmThreadPriority(index, THREAD_PRIORITY_GAMES)；反过来会拿到 ERROR_INVALID_TASK_INDEX（1551）。task index 还可能失效（服务重启），需要自愈路径（重新注册）。注册失败时退回 SetThreadPriority，并如实把用了哪种方式与错误码记进统计 | 基准实测：注入线程优先级 = MMCSS/Games，注册无报错 |
| 2026-10-30 |  | 帧头只有一个数据报带 LAST_SHARD 标志（bit1），它丢了接收端就再也判断不出帧在哪里结束 —— 即使冗余足够、其余分片都能恢复，整帧也会一直挂在重组器里最后按 E2004 丢弃。实测恢复率只有 94% 而不是 100%。修法：发送端在整帧负载最前面加 8 字节大端前缀（原始负载字节数 + 本帧的 FEC 组数），前缀随数据片一起受 FEC 保护 ⇒ 组边界不再依赖「某一个数据报有没有到」。**帧头的位宽与语义一字未改**（§5 的表没动），但**线上负载格式变了**（两端一起更新） | 恢复率扫描：丢片数 ≤ 冗余片数 ⇒ 100%；超出 ⇒ 0% 且绝不产出坏帧 |
| 2026-10-30 |  | 路径 MTU 在黑洞检测时可能掉到 1200 以下，此时 1200 字节的数据报直接发不出去。修法：max_datagram 被连接实际能承载的上限夹一刀（Conn::effective_max_datagram） | beam-net 双 feature 状态测试全绿 |
| 2026-10-30 |  | B3-A 记下「每帧一次 D3D11 纹理创建，是否落在 1.5ms/1ms 预算内未实测」。perf_span!("capture") 包住的正是这一整段，用已有探针即可测。本机 RTX 3070 / 1920×1080 / release：239 帧，均值 32.4 µs、p50/p95 25/25 µs、p99 75 µs、最大 273.4 µs，而 §3.5 预算 T1 ≤ 1.5 ms ⇒ p99 只占 5% | 数字与口径写进 crates/beam-capture/examples/perf_probe.rs 的模块文档 |
| 2026-10-30 |  | B3 记过 main 返回后 NVIDIA 驱动侧 atexit 回调访问已释放对象 → 0xC0000005。本轮做了对照实验：「先 drop 设备再让 main 正常返回」与「保持设备存活」两种写法**都是 exit 0** ⇒ 当前驱动版本下复现不出来。因此代码只改成更稳妥的写法（不再用 process::exit(0) 绕过 CRT 收尾、让设备活到进程结束），**不能声称已修复** | cargo run --release -p beam-encode --example perf_probe --features ffmpeg,perf-stats → exit 0 |
| 2026-10-31 | FEC 编码超 §3.2 预算的根因：每帧重建 RS 矩阵 | 分段实测定位：`ReedSolomon::new(k,m)` 要在 GF(2^8) 上求逆 k×k 矩阵（O(k^3)），T1 档位单次 4.894 ms，占整帧 6.635 ms 的 74%，而真正的 GF 乘加只有 0.773 ms。而 `packetize()` 每帧 `FecCodec::new()` ⇒ 矩阵缓存永不跨帧存活。改法：`FecCodec` 缓存表改成 `Arc<Mutex<HashMap>>`（克隆体共享同一张表），发送端挂 `Conn`、接收端挂 `Reassembler`，新增 `packetize_with_codec`。**帧头与线上格式一字未改**（有单测 `shared_codec_produces_identical_packets` 钉住逐字节一致） | T1 6.635 → 1.553 ms（4.3×）、T2 5.320 → 0.716 ms（7.4×）、T3 1.397 → 0.276 ms（5.1×，**进预算**）；恢复路径 T1 6.98 → 1.75 ms（−71.7%）、T3 → 0.278 ms（**进预算**）。T1/T2 仍超预算 1.8~1.9 倍，根因已是标量 GF 乘加本身（登记为待决 S14） |
| 2026-10-31 | uniffi 导出面暴露重连信息与会话汇总（§4.1 / §3.9） | 新增 `Session::reconnecting()` / `Session::summary()`。**刻意做成查询式而不是新增 `SessionEvent` 变体** —— `SessionEvent` 在 beam-session / beam-ffi / 桌面端 service_crates.rs 各有一处穷尽匹配，加变体会同时打破三处。Android 侧同步：`NativeMappings.mapReconnectingInfo` / `mapSessionSummary`、`Session` 接口新增 `reconnecting()` / `summary()`、测试替身 `GeneratedShapes.kt`、4 个新单测 | cargo check -p beam-ffi --features uniffi --all-targets exit 0；gradlew test 全绿（core-net 的 NativeMappingsTest 由 16 例增至 20 例）。顺带修掉一个死代码：Android 的断线遮罩原先只认 `SessionEvent.Reconnecting` 变体，而 Rust 侧**从不发**这个变体 ⇒ 遮罩永远不会出现；现在改由 500 ms 轮询真实重连状态驱动 |
| 2026-10-31 | 真 bug：ffmpeg D3D11VA 硬解上下文被释放两次 ⇒ 退出时堆破坏 | `FfmpegD3D11VaDecoder` 把 `AVHWDeviceContext` 的 `device` 字段指向自己 AddRef 出来的 `ID3D11Device`，同时在 Rust 结构体里**又存了一份**该设备引用。而 ffmpeg 的 `AVD3D11VADeviceContext` 头文件写明：「Deallocating the AVHWDeviceContext will always release this interface」⇒ 设备被 Release 两次 ⇒ 交换链与解码纹理都建在这个设备上，提前释放让它们踩到野指针。实测：打开硬解上下文后立刻释放 ⇒ STATUS_ACCESS_VIOLATION。改法：把那一份 AddRef 的所有权**移交给 ffmpeg**（`mem::forget`），Rust 侧不再持有 | 硬解上下文开→关不再崩；`cargo test -p beam-client --features video --test video_present -- --ignored` 通过 |
| 2026-10-31 | 真 bug：`AVBufferRef` 被 unref 两次 ⇒ use-after-free | `(*ctx).hw_device_ctx = hw.as_ptr()` 把 `AVBufferRef` **无引用地**塞给 libavcodec；`avcodec_free_context` 会 unref 它，之后 `HwDevice::drop` 又 unref 一次 ⇒ use-after-free ⇒ 实测 STATUS_HEAP_CORRUPTION。改法：交给 libavcodec 前先 `av_buffer_ref` 加一份引用（`avctx` 与 `avci` 各持一份，`avcodec_free_context` 负责归还） | 两个崩溃点（堆破坏 / 访问违例）都消除；上屏自检全绿 |
| 2026-10-31 | 桌面端客户端此前**根本没有上屏路径** | 此前 `ClientRuntime::connect_default` 用的 `D3D11Renderer` **没有绑定任何窗口**，且 `beam-render/d3d11` / `beam-decode/ffmpeg` 两个 feature 默认关闭 ⇒ 客户端即便收到帧也只会得到 `E10014`（后端未启用）。本批补上 `client-video` feature 与 `connect_to_window` 入口（建窗口 → 建交换链 → 用同一设备开硬解），桌面端只调 L3 入口 | cargo check/clippy 两种 feature 状态全绿；本机自检三步（窗口/交换链/硬解）全过 |
| 2026-10-31 | D3D11 VideoProcessor 在本机驱动上建不出输入视图（不是参数问题） | `CreateVideoProcessorInputView` 恒返回 E_INVALIDARG(0x80070057)。为排除「参数写错」，做了 96 组变体矩阵（FourCC 三种取值 × Usage 四种 × 源/目标纹理绑定各两种）—— **一组都没建出来** ⇒ 驱动侧的能力缺失。文档 §3.6 允许「D3D11 VideoProcessor **或** compute shader」，因此实现 compute shader 路径 | examples/color_vp_probe.rs 留在仓库里，换机器时可再验一次；生产路径已自动回退到 compute shader |
| 2026-10-31 | compute shader 的 RGB→NV12 已真机验证（逐字节等于 CPU 参考） | 内嵌 HLSL 用 D3DCompile 运行时编译，compute shader **直写** NV12 纹理的 Y/UV 两个平面（纹理带 VIDEO_ENCODER | UNORDERED_ACCESS ⇒ NVENC 直接吃、shader 直接写，零拷贝）。色彩矩阵 BT.709 + limited range。期间修掉两个真实缺陷：① 着色器把 UV 平面按「宽 × 半高」写（NV12 的 UV 其实是 **宽/2 × 高/2** 个 R8G8 采样）；② NV12 纹理不能靠「先写单平面纹理再拷进去」来填（带 VIDEO_ENCODER 的纹理不能参与拷贝） | Y 平面自 NV12 纹理读回与 CPU 参考逐字节一致（偏差 0）；色度经影子平面偏差 0；单帧 1080p 1.02 ms / 4K 2.10 ms |
| 2026-11-01 | §5 的 ClockSync 此前是「协议有、实现无」：两端都没发也没算 | 协议里 `ClockSync{t0,t1,t2,t3}` 一直存在，主机侧也有一句应答代码，但**客户端从不发请求、也从不算偏移** ⇒ §5「客户端通过 ClockSync 求偏移」落空，§3.13 的 E2E 延迟也就没有基准。另外发现一个隐患：应答若也走通用控制处理路径，会**再回一条 ClockSync** ⇒ 两端无限乒乓 | 客户端每 5 s 发请求、按 NTP 式算偏移并可查；单测钉住「不回声」与「5 s 节拍」 |
| 2026-11-01 | 顺带修掉一个我自己引入的时钟口径隐患 | B12 给 `Clock` 加 `now_us()` 时，`SystemClock::now_ms()` 与 `now_us()` 各自用了独立的 `static START` ⇒ 两个零点会差几微秒，破坏「微秒 = 毫秒 × 1000」的一致性，也会让时钟偏移带上一个恒定偏差。改成共用同一个零点 | 两个口径现在严格一致（`now_us()/1000 == now_ms`） |
| 2026-11-01 | §3.13 的 E2E 阶段此前没有任何地方记录 | `Stage::E2E` 与 `beam_perf::e2e::clock_based_e2e_us` 一直存在，但客户端上屏后从没算过 E2E ⇒ 时间戳路径缺了「客户端上屏时刻」这一半。现在：`FrameConsumer` 新增默认空实现的 `on_clock_sync`（会话层把偏移送给消费者）→ `DecodeRenderConsumer` 在上屏后取客户端时刻并算出 E2E 记进采集器 | E2E 时间戳路径打通；偏移 100 ms 的场景测得 E2E ≈ 200 ms（单测钉住） |
| 2026-11-01 | 真 bug：mDNS 服务类型少了结尾的点 ⇒ 广播根本注册不上去 | `SERVICE_TYPE = "_beam._udp.local"`，而 `mdns-sd` 的 `register` / `browse` 都要求服务类型以 `._tcp.local.` / `._udp.local.` **结尾**（带点），否则直接返回 `RegisterFailed`。⇒ 主机广播不出去、客户端浏览不到，**局域网发现完全不可用**。改法：保留文档口径的 `SERVICE_TYPE`，另加 `SERVICE_TYPE_FQDN`（带点）供注册与浏览使用 | 真机回环里客户端广播成功、主机浏览到了它（名字 + 指纹前 8 位）；加单测钉住 FQDN 形式 |
| 2026-11-01 | 真机回环（host_loopback）此前**从未跑通**，卡在三个地方 | ① 先 connect 后 accept：quinn 的握手要靠 accept 被推进，没人推进 ⇒ 客户端超时、主机看不到客户端证书 ⇒ 信任闸门无从记录指纹；② 最小客户端不发 Feedback ⇒ 主机 3 s 停滞判定触发；③ 客户端时限短于主机 ⇒ 客户端先断开，主机按正常断开语义进 Error 态。三个都在**示例**里（产品代码没问题） | 本机真机整链首次跑通：双方到 Streaming，客户端收到 241 帧真实画面 |
| 2026-11-01 | S1 的 `plat` 字段与 S11 的 PIN 参数此前都是「文档写了、代码没接」 | ① `plat` 只在 §3.4 的文字里，beam-discovery 的 TXT 与信任库写入路径都没接上它 —— 发现到的主机恒显示「未知平台」；Android 的 NsdManager 兜底扫描读的键名（`platform`）与实际广播的键名（`plat`）对不上，永远拿不到平台。② 桌面端 `connect(host_id)` 根本没有 PIN 参数 ⇒ 前端 4 位 PIN 输入框是纯装饰，填对填错都能连上。③ S5 裁决的前提被证伪：`E12xxx` 并非空闲，`beam-log` 已占用 `E12001`（日志初始化失败） | S1：`plat` 从 mDNS TXT 贯通到 beam-discovery → beam-ffi → Android / 桌面端 → 信任库，并修掉 Android 兜底扫描的键名 bug。S5：外壳错误迁到 `E12xxx`、`E11xxx` 归配置层独占、日志层迁到 `E16xxx`，一个码只对应一种含义。S11：`connect(host_id, pin)` 补上 PIN，Rust 侧做格式校验与尝试预算（3 次 / 锁 60 s） |
| 2026-11-01 | Windows 客户端的「解码 → 上屏」链路此前是**假成功** | B10 把 D3D11VA 硬解与原生窗口上屏的代码路径搭好了，但**从未在真机跑过**。B17 第一次真机跑就发现：解码纹理是 DXGI_FORMAT_420Opaque / 1920×1152（对齐后的 coded height），交换链是 B8G8R8A8_UNORM / 1920×1080 —— 格式与尺寸双双不匹配，于是 `present` 每一帧都走「清屏 + 照样走完整条呈现路径」的静默降级：**不报错、不警告**，会话状态正常，用户看到的是空白窗口。之前的回环验证只统计「收到的帧数」，分不清「画面上屏了」与「画面被丢在地上」 | ① `beam_render::PresentInfo` 把「真的搬纹理」与「降级清屏」分开计数，并报出两边的真实格式与尺寸；② 新增 `present_smoke` 验收门禁（它现在如实报 FAIL）；③ 交换链格式改为可选并给视频格式单开路径；④ 用对照实验**实测排除**了零拷贝（420Opaque/NV12 交换链建不出来，VideoProcessor 建不出输入视图）；⑤ 静默降级改成如实报错 E10015；⑥ 结论写进设计文档 §3.10 |
| 2026-11-01 | Windows 客户端的画面**从来没真正上过屏**（B17 定位，B18 修好） | B10 把 D3D11VA 硬解与原生窗口上屏的代码路径搭好了，但从未在真机跑过。B17 第一次真机跑就发现：解码纹理是 DXGI_FORMAT_420Opaque / 1920×1152（对齐后的 coded height），交换链是 B8G8R8A8_UNORM / 1920×1080 —— 格式与尺寸双双不匹配，于是每一帧都走「清屏 + 照样走完整条呈现路径」的静默降级：不报错、不警告，会话状态正常，用户看到的是空白窗口。B18 补上了 VideoProcessor 转换路径（4:2:0 → BGRA），画面这才真的上屏 | ① `beam_render::vp` 用 D3D11 VideoProcessor 做 4:2:0 → BGRA 转换；关键在于输入视图的 FourCC 必须用 `'420O'`（`beam-encode` 那次 96 组变体全灭，是因为源纹理是自造的 NV12/BGRA，从没试过解码器真实输出的 420Opaque）；② `PresentInfo` 增加 `converted_presents`，区分「直接搬」与「转换后搬」；③ 真机实测 9 帧全部上屏（转换 9 / 降级 0）；④ 转换也不可用时如实报 E10015，不再假装在串流；⑤ 顺带把 V-13 的丢帧统计贯通：抓屏单槽覆盖开始计数，进主机会话汇总；客户端补上渲染侧丢帧，`dropped_frames` 覆盖从抓到屏到上屏的全程 |
| 2026-11-04 | Windows 串流的**画面链路**第一次端到端真机跑通 | B17 定位到「画面从来没上过屏」，B18 用 VideoProcessor 转换修好了上屏，但那只是「解码 → 上屏」这一段。B19 把整链接通：主机侧真抓屏 + 真硬编 → QUIC → 客户端侧真硬解 + 真上屏到独立原生窗口。过程中还发现两个「验证入口本身会骗人」的坑：① WGC 是内容驱动的，屏幕静止时主机一帧都发不出来（同一台机器上帧数在 0 与 1 之间跳）；② 客户端先断开时主机会话必然进 Error 态（正常断开语义），两个时限弄反就会被误判成「主机出错」 | 新增验证入口 `crates/beam-host/examples/client_loopback.rs`：本机实测 345 帧全部经 VideoProcessor 转换后搬进交换链（降级 0）⇒ 产品方案第 45 行的「画面真的出现在窗口里」达成。顺带把三个 crate 的 `perf_span!` 占位符统一成零尺寸结构体（B18 只改了 beam-render，其余三个在 `--features wgc/ffmpeg` 组合下会被 clippy 拦下） |
| 2026-11-04 | Windows 串流的**音频链路**第一次端到端真机跑通，并修掉了一个「跑通了但速率只有 1/3」的缺陷 | B20 接上音频后立刻暴露两个问题：① 客户端用裸 `video_stream()` 收帧，而画面重组器**不看帧头 `type`** ⇒ 音频数据报会被当成画面分片组装（负载是裸 Opus 字节）；② 主机只送出 **65 帧/秒**，而 §3.7 的音频是 5 ms 一帧（实时 200 帧/秒），采集队列一路积压到丢帧。对照实验（采集 / +Opus 编码 / +建数据报 三段各测速率）证明**处理能力有 199.7 帧/秒**，瓶颈在消费循环「一轮只取一帧」—— 速率被循环的唤醒间隔限死。改成「一轮把攒下的帧全发走」后实测 800 帧 / 4 秒 = 200 帧/秒、队列零积压。分流读点也从 `beam-client` 下沉到 `beam-net::DemuxStream`（L1）：§4 规定 L3 之间不得横向依赖，而主机侧的整链验证入口也需要同一个读点 | 真机实测：音频 **800 帧走完整链**（WASAPI loopback 采集 → Opus 编码 → QUIC datagram → 按帧头 type 分流 → Opus 解码 → WASAPI 输出），错误 0 次，解码 PCM 的 RMS = 0.084（非静音）；画面同一次运行 285 帧全部经 VideoProcessor 转换后搬进交换链、降级 0。顺带把 §3.10 的渲染预算**第一次对照实测**：解码均值 0.348 ms（预算 8 ms）、上屏均值 0.106 ms（半帧预算 8.33 ms）⇒ 均达标 |

---

## 3. 分片目录

3. **beam-proto 与协议规格** —— §3.1 / §5 / §6（`part-1.md`，82 条）
4. **beam-net / beam-pair / beam-discovery** —— §3.2 / §3.3 / §3.4（`part-2.md`，66 条）
5. **平台层 crate（capture/encode/audio/input/decode/render）** —— §3.5 / §3.6 / §3.7 / §3.8 / §3.10（`part-3.md`，93 条）
6. **session / host / client / ffi / gamestream** —— §3.9 / §4.1 / §3.12 uniffi / §2（`part-4.md`，65 条）
7. **Android 12 模块** —— §3.12 / §7 Android / 产品方案 §7 §16（`part-5.md`，109 条）
8. **Tauri 桌面端** —— §3.11 / §2 / 产品方案 §6 §15 §16（`part-6.md`，85 条）
9. **横切面（分层 / 构建发布 / 测试验收 / 里程碑）** —— §2 / §4 / §7 / §8 / §9 / §3.13 / §3.14 / §3.15（`part-7.md`，51 条）

---

## 4. 条目明细

### 4. beam-proto 与协议规格（§3.1 / §5 / §6）

> 来自 `part-1.md`

#### 条目

##### §3.1 beam-proto

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| P1-01 | 职责:所有线上消息与帧头的定义、序列化(postcard)、版本协商。 | `crates/beam-proto/src/message.rs:69`、`crates/beam-proto/src/frame.rs:117`、`crates/beam-proto/src/caps.rs:196` | 已实现 | 消息枚举 `ControlMsg`（message.rs:70-136）、帧头 `VideoPktHeader`（frame.rs:119-136）、版本协商 `negotiate_version`（caps.rs:197-211）均为真实实现，序列化确实走 postcard（message.rs:207、250）。 | 未开始 |
| P1-02 | 边界:只含类型与编解码;不做 IO。 | `crates/beam-proto/src/lib.rs:56-61`、`crates/beam-proto/Cargo.toml:11-18` | 已实现 | 模块只有 caps/error/frame/input/lib/message/stats；依赖仅 serde/postcard/thiserror/bytes，全 crate 无任何 IO 调用。 | 未开始 |
| P1-03 | 依赖 serde/postcard | `crates/beam-proto/Cargo.toml:12-13` | 已实现 | `serde = { workspace = true }`、`postcard = { workspace = true }` 逐字命中。 | 未开始 |
| P1-04 | 被 net/session/ffi 依赖 | `crates/beam-net/Cargo.toml:12`、`crates/beam-session/Cargo.toml:12`、`crates/beam-ffi/Cargo.toml:17` | 已实现 | 另有 pair/discovery/config/encode 等 11 个 crate 依赖 beam-proto，未发现反向依赖。 | 未开始 |
| P1-05 | 禁止依赖任何平台 crate 或 tokio | `crates/beam-proto/Cargo.toml:11-18` | 已实现 | 依赖表无 `windows*`、`tokio`、`quinn`；亦无 `[features]` 门控。 | 未开始 |
| P1-06 | §2 目录注释:`beam-proto/` # 消息、帧头、版本协商(**no_std 友好**) | `crates/beam-proto/src/lib.rs:54`、`crates/beam-proto/src/message.rs:56`、`crates/beam-proto/Cargo.toml:15` | 偏差 | crate 未做 no_std 适配：无 `#![no_std]`（lib.rs:54 只有 `forbid(unsafe_code)`）；`Feedback.rtt` 用 `std::time::Duration`（message.rs:56）；postcard 工作区开 `use-std` feature（根 `Cargo.toml:41`），`bytes` 亦为 std 版（根 `Cargo.toml:67`）。 | 未开始 |
| P1-07 | 接口:`fn encode<M: Message>(m:&M, buf:&mut BytesMut)` | `crates/beam-proto/src/message.rs:206` | 偏差 | 参数与泛型逐字一致，但返回类型由文档隐含的 `()` 变为 `Result<(), ProtoError>`（message.rs:206）。副作用：编码失败被映射成 `ProtoError::DecodeFailed`（message.rs:207-209），语义上把「编码失败」并入 E1002「解码失败」。 | 未开始 |
| P1-08 | 接口:`fn decode(buf:&[u8]) -> Result<AnyMsg, ProtoError>` | `crates/beam-proto/src/message.rs:230` | 已实现 | 签名逐字一致；按 1 字节族标签分派（message.rs:231-240）。 | 未开始 |
| P1-09 | 接口:`fn negotiate(local:&Caps, remote:&Caps) -> Result<Agreed, ProtoError>` | `crates/beam-proto/src/caps.rs:214` | 已实现 | 签名逐字一致；取公共编解码器最大值与分辨率/帧率最小值（caps.rs:215-237）。 | 未开始 |
| P1-10 | 数据:`Hello{ver,caps}` | `crates/beam-proto/src/message.rs:40-45` | 已实现 | 字段名与字段集逐字一致（`ver: ProtocolVersion`、`caps: Caps`）。 | 未开始 |
| P1-11 | 数据:`Caps{codecs,max_res,max_fps,features}` | `crates/beam-proto/src/caps.rs:113-122` | 已实现 | 字段名与字段集逐字一致（`codecs: Vec<CodecKind>`、`max_res: Resolution`、`max_fps: u32`、`features: u64`）。 | 未开始 |
| P1-12 | 数据:`StreamConfig` | `crates/beam-proto/src/caps.rs:164-179` | 已实现 | 结构体存在且被 `ControlMsg::StreamConfig` 使用（message.rs:101）；文档未规定其字段集，故只核对存在性与可用性。 | 未开始 |
| P1-13 | 数据:`InputEvent` | `crates/beam-proto/src/input.rs:49-96` | 已实现 | 6 个变体（鼠标移动/按键/键盘/滚轮/摇杆/手柄按键），整数定点表示，平台无关。 | 未开始 |
| P1-14 | 数据:`StatsReport` | `crates/beam-proto/src/stats.rs:9-28` | 已实现 | 9 个统计字段，全部整数（微秒/百万分比）。 | 未开始 |
| P1-15 | 数据:`VideoPktHeader`(见第 5 章) | `crates/beam-proto/src/frame.rs:119-136` | 已实现 | 8 个字段与 §5 帧头表一一对应（`type` 因是 Rust 关键字改名为 `kind`，线上位型不变，见 P5-22）。 | 未开始 |
| P1-16 | 线程:纯函数。 | `crates/beam-proto/src/message.rs:206`、`crates/beam-proto/src/message.rs:230`、`crates/beam-proto/src/caps.rs:214` | 已实现 | 三个公开接口均为无状态纯函数，crate 内无 `Arc`/`Mutex`/线程/异步。 | 未开始 |
| P1-17 | 错误码:`E1xxx` 1001 版本不兼容 | `crates/beam-proto/src/error.rs:10`、`crates/beam-proto/src/error.rs:42-48` | 已实现 | `VERSION_INCOMPATIBLE = 1001`，由 `negotiate_version` 触发（caps.rs:201-206）。 | 未开始 |
| P1-18 | 错误码:`E1xxx` 1002 解码失败 | `crates/beam-proto/src/error.rs:12`、`crates/beam-proto/src/error.rs:51-55` | 已实现 | `DECODE_FAILED = 1002`；postcard 失败（message.rs:250-252）与帧头长度不足（frame.rs:172-176）都映射到该码。 | 未开始 |
| P1-19 | 错误码:`E1xxx` 1003 未知消息 | `crates/beam-proto/src/error.rs:14`、`crates/beam-proto/src/error.rs:58-62` | 已实现 | `UNKNOWN_MESSAGE = 1003`；未知族标签（message.rs:236-238）与族标签不匹配（message.rs:222-224）均返回该码。 | 未开始 |
| P1-20 | 配置:无。 | `crates/beam-proto/Cargo.toml:11-18`、`crates/beam-proto/src/lib.rs:56-61` | 已实现 | 无配置模块、无 `[features]`、无配置项类型。 | 未开始 |
| P1-21 | 性能:单消息编解码 < 2 µs | 未找到（已搜索：`criterion`、`[[bench]]`、`benches` 目录、`#[bench]`、`fn bench`、`divan`、`iai`；全仓库零匹配） | 缺失 | 仓库内没有任何编解码基准，`< 2 µs` 预算无法被验证。`crates/beam-proto/Cargo.toml:17-18` 的 dev-dependencies 只有 proptest。 | 已完成 |
| P1-22 | 测试:proptest 往返 | `crates/beam-proto/tests/roundtrip.rs:15-74` | 已实现 | 两组 proptest：帧头逐位往返（roundtrip.rs:17-42）与控制消息 postcard 往返（roundtrip.rs:45-73），另有枚举序号稳定性断言（roundtrip.rs:77-83）。 | 未开始 |
| P1-23 | 测试:模糊测试 cargo-fuzz | 未找到（已搜索：仓库根无 `fuzz/` 目录，`Cargo.toml` 无 `cargo-fuzz` 配置，无 `libfuzzer`/`arbitrary` 依赖） | 缺失 | 无模糊测试目标。`crates/beam-proto/tests/roundtrip.rs:1` 自述只覆盖 proptest 往返。 | 已完成 |
| P1-24 | 测试:版本兼容快照 | 未找到（已搜索：`insta`、`assert_snapshot`、仓库内无 `snapshots/` 目录） | 缺失 | 无快照测试，线上字节序列没有被固定下来。现有最接近的是前向兼容单测 `crates/beam-proto/src/message.rs:299-307`（尾部字节被忽略），只覆盖“末尾追加”，不覆盖“旧版本解码新版本字段”。 | 已完成 |
| P1-25 | 里程碑:MVP | `crates/beam-proto/src/lib.rs:54`、`crates/beam-proto/Cargo.toml:1-21` | 偏差 | 类型与编解码部分已达 MVP 形态，但 §3.1 为 MVP 规定的三项测试只完成 proptest 一项（cargo-fuzz、版本兼容快照缺失），且无性能基准，MVP 验收条件未闭合。 | 未开始 |
| P1-26 | （文档未要求）能力协商失败码 | `crates/beam-proto/src/error.rs:19`、`crates/beam-proto/src/error.rs:65-69` | 超出 | §3.1 只定义 1001–1003 三码；代码新增 `E1004 CapabilityMismatch`（error.rs:19、65-69），并由 `negotiate` 在无公共编解码器/帧率为 0 时返回（caps.rs:221-228）。风险：占用 E1xxx 段码位且未经文档授权，后续文档若给 1004 分配别的语义会静默冲突。 | 未开始 |

##### §5 协议规格

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| P5-01 | ALPN `beam/1` | `crates/beam-proto/src/message.rs:17` | 已实现 | `pub const ALPN: &[u8] = b"beam/1";`，并在 TLS 配置里实际使用（`crates/beam-net/src/endpoint.rs:61`、`crates/beam-net/src/endpoint.rs:92`）。 | 未开始 |
| P5-02 | 控制 stream 0(双向可靠) | `crates/beam-proto/src/message.rs:19-27`、`crates/beam-net/src/endpoint.rs:174-181`、`crates/beam-client/src/adapters.rs:82-85` | 偏差 | 协议层用 1 字节“消息族标签”表达通道（message.rs:19-27、211），没有 stream 号概念；QUIC 层提供 `open_bi`（endpoint.rs:174-181）但**全仓库无调用方**（已搜索 `.open_bi(`、`.open_uni(`，仅命中定义处）；会话侧真实实现 `NetTransport::send_control` 是空实现，注释「本轮只保留契约」（adapters.rs:82-85），`recv_control` 直接返回 `None`（adapters.rs:101-103）。通道未接通。 | 已完成 |
| P5-03 | 输入 stream(单向可靠,客户端→主机) | `crates/beam-proto/src/message.rs:26`、`crates/beam-proto/src/input.rs:49`、`crates/beam-net/src/endpoint.rs:183-188`、`crates/beam-client/src/adapters.rs:96-99` | 偏差 | 协议层类型齐备（族标签 `INPUT`=2，message.rs:26；`InputEvent` input.rs:49），单向流原语 `open_uni` 存在但无调用方；`NetTransport::send_input` 为空实现（adapters.rs:96-99）。 | 已完成 |
| P5-04 | 视频/音频走 QUIC datagram | `crates/beam-proto/src/frame.rs:31-40`、`crates/beam-net/src/endpoint.rs:203-207`、`crates/beam-net/src/datagram.rs:60-139` | 已实现 | 帧头 `type` 区分视频/音频/FEC（frame.rs:31-40）；net 层有 `send_datagram`（endpoint.rs:203-207）与分包实现（datagram.rs:60-139）。 | 未开始 |
| P5-05 | 控制消息:Hello | `crates/beam-proto/src/message.rs:72`、`crates/beam-proto/src/message.rs:40-45` | 已实现 | `ControlMsg::Hello(Hello)`。 | 未开始 |
| P5-06 | 控制消息:HelloAck | `crates/beam-proto/src/message.rs:74-77` | 已实现 | `HelloAck { agreed: Agreed }`。 | 未开始 |
| P5-07 | 控制消息:PairRequest | `crates/beam-proto/src/message.rs:79-82` | 已实现 | `PairRequest { token: String }`。 | 未开始 |
| P5-08 | 控制消息:PairChallenge | `crates/beam-proto/src/message.rs:84-87` | 已实现 | `PairChallenge { msg: Vec<u8> }`（SPAKE2 首条）。 | 未开始 |
| P5-09 | 控制消息:PairOk | `crates/beam-proto/src/message.rs:89-92` | 已实现 | `PairOk { fingerprint: [u8; 32] }`。 | 未开始 |
| P5-10 | 控制消息:PairFail | `crates/beam-proto/src/message.rs:94-99` | 已实现 | `PairFail { code: u16, reason: String }`。 | 未开始 |
| P5-11 | 控制消息:StreamConfig | `crates/beam-proto/src/message.rs:101`、`crates/beam-proto/src/caps.rs:164-179` | 已实现 | `StreamConfig(StreamConfig)`。 | 未开始 |
| P5-12 | 控制消息:RequestIdr | `crates/beam-proto/src/message.rs:103` | 已实现 | `RequestIdr`。 | 未开始 |
| P5-13 | 控制消息:SetBitrate | `crates/beam-proto/src/message.rs:105-108` | 已实现 | `SetBitrate { bps: u32 }`。 | 未开始 |
| P5-14 | 控制消息:Feedback | `crates/beam-proto/src/message.rs:110`、`crates/beam-proto/src/message.rs:52-59` | 已实现 | `Feedback(Feedback)`，字段 `loss/rtt/recv_rate` 与 §3.2 一致。 | 未开始 |
| P5-15 | 控制消息:ClockSync(t0..t3) | `crates/beam-proto/src/message.rs:112-121` | 已实现 | 4 个 u64 时间戳 `t0/t1/t2/t3` 齐备。 | 未开始 |
| P5-16 | 控制消息:StatsReport | `crates/beam-proto/src/message.rs:123`、`crates/beam-proto/src/stats.rs:9-28` | 已实现 | `StatsReport(StatsReport)`。 | 未开始 |
| P5-17 | 控制消息:Resume | `crates/beam-proto/src/message.rs:125-128` | 已实现 | `Resume { session_id: u64 }`。 | 未开始 |
| P5-18 | 控制消息:Bye | `crates/beam-proto/src/message.rs:130` | 已实现 | `Bye`。 | 未开始 |
| P5-19 | 控制消息:Error{code} | `crates/beam-proto/src/message.rs:132-135` | 已实现 | `Error { code: u16 }`。 | 未开始 |
| P5-20 | 帧头(视频 datagram,16 字节,大端) | `crates/beam-proto/src/frame.rs:157-168`、`crates/beam-proto/src/frame.rs:171-189` | 已实现 | `encode()` 写 16 字节、`decode()` 读回，字段偏移 0/1/2..6/6..8/8/9/10..16 与帧头表一致；逐位快照测试 `crates/beam-proto/src/frame.rs:211-241`。 | 未开始 |
| P5-21 | 帧头 `ver` 4 位，当前 1 | `crates/beam-proto/src/frame.rs:121`、`crates/beam-proto/src/frame.rs:140`、`crates/beam-proto/src/frame.rs:159` | 已实现 | 高 4 位存版本，`VER = 1`，写入时 `& 0x0F`。 | 未开始 |
| P5-22 | 帧头 `type` 4 位:0 视频 1 音频 2 FEC 冗余 | `crates/beam-proto/src/frame.rs:31-61`、`crates/beam-proto/src/frame.rs:159` | 已实现 | `FrameKind{Video=0,Audio=1,Fec=2}`，低 4 位。字段名由 `type` 改为 `kind`（Rust 关键字），线上位型不变。 | 未开始 |
| P5-23 | 帧头 `flags` 8 位:bit0 关键帧,bit1 帧末片,bit2 切片边界 | `crates/beam-proto/src/frame.rs:70-74`、`crates/beam-proto/src/frame.rs:160` | 已实现 | `KEYFRAME=1<<0`、`LAST_SHARD=1<<1`、`SLICE_BOUNDARY=1<<2`，整字节写入。 | 未开始 |
| P5-24 | 帧头 `frame_id` 32 位，递增 | `crates/beam-proto/src/frame.rs:127`、`crates/beam-proto/src/frame.rs:161` | 已实现 | 4 字节大端；递增由上层维护（net 层用 `frame.frame_id`，`crates/beam-net/src/datagram.rs:128`）。 | 未开始 |
| P5-25 | 帧头 `shard_idx` 16 位，**组内分片号** | `crates/beam-proto/src/frame.rs:129`、`crates/beam-proto/src/frame.rs:162`、`crates/beam-net/src/datagram.rs:8-9`、`crates/beam-net/src/datagram.rs:129` | 偏差 | 线上是 16 位大端（位宽符合），但**语义不同**：实现写的是帧内全局分片号 `shard_idx = global = g*(k+m)+local`（datagram.rs:108、129），组号需由接收端自己算 `shard_idx/(k+m)`、组内号取 `shard_idx%(k+m)`（datagram.rs:8-9）。与文档“组内分片号”不一致，会影响跨实现互操作。 | 已完成 |
| P5-26 | 帧头 `data_shards` 8 位 = k | `crates/beam-proto/src/frame.rs:131`、`crates/beam-proto/src/frame.rs:163` | 已实现 | 单字节写入，写入值为 FEC 组数据片数 k（`crates/beam-net/src/datagram.rs:130`）。 | 未开始 |
| P5-27 | 帧头 `fec_shards` 8 位 = m | `crates/beam-proto/src/frame.rs:133`、`crates/beam-proto/src/frame.rs:164` | 已实现 | 单字节写入，写入值为冗余片数 m（`crates/beam-net/src/datagram.rs:131`）。 | 未开始 |
| P5-28 | 帧头 `capture_ts` 48 位（主机微秒时钟） | `crates/beam-proto/src/frame.rs:27`、`crates/beam-proto/src/frame.rs:135`、`crates/beam-proto/src/frame.rs:165-166` | 已实现 | 48 位掩码 `CAPTURE_TS_MASK`，高 48 位大端写入 out[10..16]；超范围自动截断（frame.rs:152、165-166）。 | 未开始 |
| P5-29 | 帧头 `payload` ≤1184 B | `crates/beam-proto/src/frame.rs:24`、`crates/beam-net/src/config.rs:96-101`、`crates/beam-net/src/datagram.rs:66` | 已实现 | proto 给出常量 `MAX_PAYLOAD_LEN = 1184`；net 层按 `max_datagram(1200) - FRAME_HEADER_LEN(16)` 推导净负载。注意 `MAX_PAYLOAD_LEN` 在仓库内除再导出（`crates/beam-proto/src/lib.rs:69`）外无任何引用，与 net 的 `max_datagram-16` 形成两个真值来源，存在漂移风险。 | 未开始 |
| P5-30 | FEC:按帧 Reed-Solomon GF(2^8),k ≤ 200 | `crates/beam-net/src/fec.rs:19`、`crates/beam-net/src/fec.rs:58-69` | 已实现 | 实现位于 beam-net（非本次审计 crate）。`MAX_DATA_SHARDS = 200`，超限返回错误。 | 未开始 |
| P5-31 | FEC:m = ceil(k×ratio) | `crates/beam-net/src/fec.rs:24-31` | 已实现 | `parity_shard_count()` 用 `(k as f32 * ratio).ceil()`，并夹到 `k+m ≤ 255`。 | 未开始 |
| P5-32 | FEC:默认 ratio 0.2 | `crates/beam-net/src/config.rs:19`、`crates/beam-net/src/config.rs:81` | 已实现 | `DEFAULT_FEC_RATIO = 0.2`，进入 `NetConfig::default()`。 | 未开始 |
| P5-33 | FEC:码率 >150 Mbps 降到 0.1 | `crates/beam-net/src/config.rs:13`、`crates/beam-net/src/config.rs:25`、`crates/beam-net/src/config.rs:109-117` | 已实现 | `HIGH_BITRATE_BPS = 150_000_000`，`adaptive_fec_ratio` 命中。 | 未开始 |
| P5-34 | FEC:丢包 >2% 升到 0.3 | `crates/beam-net/src/config.rs:16`、`crates/beam-net/src/config.rs:28`、`crates/beam-net/src/config.rs:110-111` | 已实现 | `HIGH_LOSS_PPM = 200`（2%）；文档未规定与高码率规则的优先级，实现让丢包规则优先（config.rs:110-113）。 | 未开始 |
| P5-35 | FEC:单帧超过 255 片时拆成多个组 | `crates/beam-net/src/datagram.rs:77-80`、`crates/beam-net/src/fec.rs:19-22` | 偏差 | 实现按 **k ≤ 200** 拆组：`group_capacity = MAX_DATA_SHARDS(200) * max_payload`（datagram.rs:77-80），255 只用于限制 `k+m`（fec.rs:21-22、30）。文档自身同时写了「k ≤ 200」和「超过 255 片拆多组」两条互斥规则，代码选了 200；需文档统一或改代码。 | 已完成 |
| P5-36 | 版本协商:Hello 带 `proto_ver` 和 `features` 位图 | `crates/beam-proto/src/message.rs:40-45`、`crates/beam-proto/src/caps.rs:71-76`、`crates/beam-proto/src/caps.rs:113-122`、`crates/beam-proto/src/caps.rs:98-109` | 已实现 | `Hello.ver: ProtocolVersion{major,minor}`（caps.rs:71-76）承担 `proto_ver`；`features` 位图在 `Caps.features: u64`（caps.rs:121），按 §3.1 的字段集嵌在 `caps` 内（而非 §5 字面写的 Hello 顶层字段），位定义 `caps.rs:98-109`。 | 未开始 |
| P5-37 | 版本协商:取双方最大公共主版本 | `crates/beam-proto/src/caps.rs:201-210` | 已实现 | 主版本必须相等（否则走 P5-38 拒绝），返回的 `major` 即共同主版本；次版本取双方较小值（caps.rs:209）——文档未规定次版本取大还是取小。 | 未开始 |
| P5-38 | 版本协商:主版本不同直接拒绝(E1001) | `crates/beam-proto/src/caps.rs:201-206`、`crates/beam-proto/src/error.rs:42-48` | 已实现 | 返回 `ProtoError::VersionIncompatible`，`code() == 1001`（error.rs:76）。 | 未开始 |
| P5-39 | 版本协商:次版本向后兼容 | `crates/beam-proto/src/caps.rs:207-210`、`crates/beam-proto/src/message.rs:248-254` | 已实现 | 次版本取 min；解码用 `postcard::take_from_bytes`，尾部多余字节被丢弃（message.rs:249-253）。 | 未开始 |
| P5-40 | 版本协商:未知字段忽略 | `crates/beam-proto/src/message.rs:248-254`、`crates/beam-proto/src/message.rs:299-307` | 已实现 | postcard 非自描述格式 + serde derive 默认忽略未知字段；尾部追加有单测覆盖（message.rs:299-307）。未见“旧版本解码新版本枚举变体”的用例（postcard 枚举未知变体会失败）。 | 未开始 |
| P5-41 | 版本协商:postcard 末尾追加 | `crates/beam-proto/src/message.rs:248-254` | 已实现 | `take_from_bytes` 返回 `(值, 剩余)`，剩余被丢弃（message.rs:249-252）；单测 `message.rs:299-307` 验证追加 3 字节仍能解回。 | 未开始 |
| P5-42 | 兼容策略:v1 GameStream 兼容层作为独立 crate `beam-gamestream`,只按公开协议行为实现,不参考 GPL 代码 | `crates/beam-gamestream/Cargo.toml:10-13`、`crates/beam-gamestream/src/lib.rs:12-21` | 占位 | feature 门控：`gamestream`（Cargo.toml:11-13，默认关）。crate 自述「本轮只交付**最小骨架**:能力映射表 + 特性开关 + 占位错误码,不含任何真实握手报文」（lib.rs:20-21）；净室声明见 lib.rs:5-10。 | 未开始 |

##### §6 跨模块约定

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| P6-01 | 时间:所有时间戳为单调时钟微秒 `u64` | `crates/beam-proto/src/lib.rs:84-86`、`crates/beam-proto/src/lib.rs:112-116`、`crates/beam-proto/src/frame.rs:135`、`crates/beam-proto/src/message.rs:114-120` | 已实现 | `EncodedFrame.pts_us/capture_ts_us: u64`、`ClockSync.t0..t3: u64`、`VideoPktHeader.capture_ts: u64` 均符合。注意文档自身冲突：§5 要求帧头 `capture_ts` 为 48 位，代码按 §5 截断（frame.rs:152、165-166），48 位微秒约 8.9 年不回绕；`StatsReport` 的 `rtt_us/e2e_us` 等是时长而非时间戳（stats.rs:19-25，u32）。 | 未开始 |
| P6-02 | 主机抓屏时刻为帧的 capture_ts | `crates/beam-proto/src/lib.rs:85-86`、`crates/beam-proto/src/lib.rs:114-116`、`crates/beam-proto/src/lib.rs:152-153`、`crates/beam-proto/src/frame.rs:135` | 已实现 | `EncodedFrame.capture_ts_us`、`AssembledFrame.capture_ts_us`、`DecodedFrame.capture_ts_us` 与帧头 `capture_ts` 贯通。 | 未开始 |
| P6-03 | 客户端通过 ClockSync 求偏移 | 未找到（已搜索：`clock_offset`、`offset_us`、`sync_offset`、`fn .*offset`、`estimate`、`ClockSync` 的消费方；全仓库仅 `crates/beam-proto/src/message.rs:112-121` 定义该消息，无任何求偏移算法） | 缺失 | 只有消息定义，没有 NTP 式四时间戳偏移计算（也缺 §3.13 要求的每 5 s 一次调度）。 | 已完成 |
| P6-04 | 内存所有权:EncodedFrame 用 `Bytes` 共享切片 | `crates/beam-proto/src/lib.rs:80-91`、`crates/beam-proto/src/lib.rs:95-105` | 已实现 | `slices: Vec<bytes::Bytes>`，`to_payload()` 拼接零拷贝（lib.rs:95-105）。位置偏差：文档 §3.2 把 `EncodedFrame` 归在 beam-net，实现放在 beam-proto（理由见 lib.rs:75-78：§4 禁止 encode→net 的 L1 横向依赖），字段名与 §3.2 逐字一致。 | 未开始 |
| P6-05 | 内存所有权:D3D11 纹理用引用计数 COM 指针,capture→encode 同一设备上零拷贝 | `crates/beam-capture/src/source.rs:7-9`、`crates/beam-encode/src/encoder.rs:16` | 缺失 | 只在两个平台 crate 各有一个 `Texture = ID3D11Texture2D` 类型别名与一句注释；未找到任何设备创建/共享代码（已搜索：`ID3D11Texture2D`、`Clone for Texture`、`same_device`、`CreateDevice`、`零拷贝` → 仅命中类型别名与文档注释）。跨设备零拷贝未实现。 | 未开始 |
| P6-06 | 内存所有权:JNI 热路径 DirectByteBuffer 由 Rust 侧池化,回调返回后归还 | `crates/beam-ffi/src/lib.rs:33-47`、`crates/beam-ffi/src/client.rs:68-73` | 缺失 | 只有设计说明：「本轮先落地接口契约与回调桥接,DirectByteBuffer 池在 M1（MVP）随 Android `core-net` 一起实现」（lib.rs:46-47）；`FrameSink::on_frame(&mut self, data: &[u8], ...)` 收切片而非 ByteBuffer 句柄（client.rs:68-73）。已搜索 `DirectByteBuffer`/`JNIEnv`/`ByteBuffer`/`pool`/`recycle` → 无池实现。 | 未开始 |
| P6-07 | 错误传播:各 crate 定义 thiserror 枚举并映射到 `E` 码 | `crates/beam-proto/src/error.rs:39-70`、`crates/beam-proto/src/error.rs:72-102` | 已实现 | proto 用 `thiserror::Error`（error.rs:39），并提供 `code()/code_str()/text()/retryable()`（error.rs:74-101）；其它 crate 同构（`crates/beam-net/src/error.rs`、`crates/beam-session/src/error.rs`、`crates/beam-config/src/error.rs:79`、`crates/beam-log/src/error.rs`）。 | 未开始 |
| P6-08 | 错误传播:session 汇总为 `SessionEvent::Error{code, retryable}` | `crates/beam-session/src/event.rs:31-37`、`crates/beam-session/src/host.rs:87-99`、`crates/beam-session/src/client.rs:122-134` | 已实现 | 变体字段 `code: u16` + `retryable: bool` 逐字一致；主机侧与客户端侧都从底层错误映射后 emit。 | 未开始 |
| P6-09 | 错误传播:UI 只根据 code 显示文案 | `crates/beam-proto/src/error.rs:25-36`、`crates/beam-proto/src/error.rs:88-91`、`crates/beam-session/src/event.rs:44-48` | 已实现 | `message(code)` 反查中文文案（含未知码兜底 error.rs:31、36），`ProtoError::text()` 供 UI 使用。 | 未开始 |
| P6-10 | 日志(§3.15):字段规范 `session_id`、`peer`、`stage`、`code` | `crates/beam-log/src/fields.rs:7-16`、`crates/beam-log/src/fields.rs:49-64` | 已实现 | `LogFields` 四字段齐备，`to_pairs()` 输出固定键名，`code` 渲染成 `E{code}`（fields.rs:61）。 | 未开始 |
| P6-11 | 日志(§3.15):禁止记录 PIN、私钥、token | `crates/beam-log/src/redact.rs:10-68` | 已实现 | `Sensitive<T>` 的 Debug/Display 只输出掩码（redact.rs:35-45），另有 `redact_pin/redact_token/redact_secret`（redact.rs:48-60）。 | 未开始 |
| P6-12 | 日志(§3.15):热路径只允许 `trace!` 且默认关闭 | `crates/beam-log/src/init.rs:23-24`、`crates/beam-log/src/init.rs:40` | 已实现 | 默认级别 `INFO`，`trace` 默认关闭（init.rs:23-24、40）。说明：「只允许 trace!」目前只是约定，crate 内没有编译期强制（未见 lint/断言）。 | 未开始 |
| P6-13 | 日志(§3.15):JSON 文件轮转(7 天/50 MB) | `crates/beam-log/src/rolling.rs:15`、`crates/beam-log/src/rolling.rs:18`、`crates/beam-log/src/rolling.rs:21`、`crates/beam-log/src/init.rs:92` | 已实现 | `DEFAULT_MAX_BYTES = 50*1024*1024`、`DEFAULT_MAX_DAYS = 7`、最多 16 个文件；输出层用 `.json()`（init.rs:92）。 | 未开始 |
| P6-14 | 配置优先级(§3.14):命令行/环境变量 > 会话内临时覆盖 > 用户配置文件 > 设备能力推导默认值 > 内置默认 | `crates/beam-config/src/model.rs:340-350`、`crates/beam-config/src/model.rs:352-412` | 已实现 | 五层顺序逐字一致（`cli, env, session, file, device`，model.rs:364-370），逐字段取最高优先级，最后回落内置默认（model.rs:377-411）。 | 未开始 |

---

#### 文档未覆盖但代码里有的

1. **线上消息的 1 字节族标签前缀**：`crates/beam-proto/src/message.rs:19-27`、`crates/beam-proto/src/message.rs:211`。文档只说「序列化(postcard)」，未规定前缀格式；这是实际线上格式的一部分，改动即破坏兼容性，需补进 §5。
2. **`decode_as<M>(buf) -> Result<M, ProtoError>`**：`crates/beam-proto/src/message.rs:219`。文档只要求 `decode(buf) -> Result<AnyMsg, ProtoError>`；额外 API 本身合理（proptest 依赖它），但它是文档外的新增公开契约。
3. **E1004 码位**：`crates/beam-proto/src/error.rs:19`、`crates/beam-proto/src/error.rs:65-69`（详见 P1-26）。
4. **proto 层额外承载的跨 crate 结构**：`EncodedFrame` / `AssembledFrame` / `DecodedFrame` / `PlatformHandle`（`crates/beam-proto/src/lib.rs:75-162`）。文档把它们分别放在 §3.2（beam-net）与 §3.10（beam-decode/beam-render），实现统一放 L0 协议层。分层理由写在 lib.rs:75-78 与 lib.rs:124-128，但文档 §3.1/§3.2 的数据归属未同步更新。
5. **`MAX_PAYLOAD_LEN` 常量无消费方**：`crates/beam-proto/src/frame.rs:24`、`crates/beam-proto/src/lib.rs:69`。仓库内没有第二处引用，net 层另算 `max_datagram - 16`（`crates/beam-net/src/config.rs:98-101`），存在两处真值。
6. **无条件暴露的 v2 能力位**：`CodecKind::Av1`（`crates/beam-proto/src/caps.rs:23`）与 `feature::HDR`（`crates/beam-proto/src/caps.rs:108`）。文档 §1.3/§7 把 AV1、HDR 归为 v2 能力，协议层没有对应的 feature 门控，能力位在 MVP 构建里同样可被声明。

#### 最该优先补的（最多 3 条）

1. **§5 三通道与帧头语义对齐**（P5-02、P5-03、P5-25）：协议类型已经齐备，但控制流/输入流在 `crates/beam-client/src/adapters.rs:82-99` 是空实现、`recv_control`/`recv_frame` 直接返回 `None`，端到端跑不通；同时 `shard_idx` 实现为帧内全局序号而文档写「组内分片号」（`crates/beam-net/src/datagram.rs:129`），必须现在定稿，否则两端实现会互不兼容。
2. **§3.1 的测试三缺**（P1-21、P1-23、P1-24）：没有基准就无法证明「单消息编解码 < 2 µs」，没有 fuzz 就没有解码器健壮性保障，没有快照就无法保证线上格式不被无意改动。建议最少补三件：`cargo-fuzz`（或 proptest 的退化/对抗属性）目标、一个固定线上字节序列的快照测试、一个编解码基准（criterion 或 divan）。
3. **§6 时钟同步与 JNI 缓冲池未实现**（P6-03、P6-06）：`ClockSync{t0..t3}` 只有消息定义，全仓库没有任何求偏移的算法（已搜索 `clock_offset`/`offset_us`/`fn .*offset` 等），E2E 延迟无法据此换算；JNI DirectByteBuffer 池在 `crates/beam-ffi/src/lib.rs:46-47` 被明确推迟到 M1，热路径仍是切片拷贝。
### 5. beam-net / beam-pair / beam-discovery（§3.2 / §3.3 / §3.4）

> 来自 `part-2.md`

#### 条目

##### §3.2 beam-net

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| N-01 | 依赖 proto、pair(验证器 trait)、quinn、reed-solomon-erasure | crates/beam-net/Cargo.toml:12-17 | 已实现 | beam-proto / beam-pair / quinn / reed-solomon-erasure 四个依赖齐备；`PeerVerifier` 由 beam-pair 提供并在 crates/beam-net/src/endpoint.rs:14 引入 | 未开始 |
| N-02 | 禁止依赖 capture/encode/decode | crates/beam-net/Cargo.toml:11-21 | 已实现 | 依赖块内无 beam-capture / beam-encode / beam-decode；未找到对这三个 crate 的引用 | 未开始 |
| N-03 | `pub async fn listen(cfg: NetConfig, verifier: Arc<dyn PeerVerifier>) -> Result<Self, NetError>` | crates/beam-net/src/endpoint.rs:52 | 已实现 | 签名逐字一致；真建 TLS/QUIC 服务端配置并 bind（crates/beam-net/src/endpoint.rs:55-74） | 未开始 |
| N-04 | `pub async fn connect(addr: SocketAddr, cfg: NetConfig, verifier: Arc<dyn PeerVerifier>) -> Result<Conn, NetError>` | crates/beam-net/src/endpoint.rs:78-82 | 已实现 | 签名逐字一致；含 tokio 超时包装（crates/beam-net/src/endpoint.rs:113-118） | 未开始 |
| N-05 | `trait VideoSender { fn send_frame(&self, f: EncodedFrame) -> Result<(), NetError>; }` | crates/beam-net/src/endpoint.rs:23-26 | 已实现 | trait 签名逐字一致；实现见 crates/beam-net/src/endpoint.rs:253-261 | 未开始 |
| N-06 | `trait VideoReceiver { async fn next_frame(&mut self) -> Result<AssembledFrame, NetError>; }` | crates/beam-net/src/endpoint.rs:29-32（实现 crates/beam-net/src/endpoint.rs:277-288） | 已实现 | 用 `fn ... -> impl Future<Output=...> + Send` 书写，这是 `async fn` 在 trait 中的等价脱糖形式，调用侧同样 `.await`；无桩实现 | 未开始 |
| N-07 | `pub struct RateController { pub fn on_feedback(&mut self, fb: &Feedback) -> BitrateDecision; }` | crates/beam-net/src/rate.rs:98-154 | 已实现 | 签名逐字一致；EWMA 丢包/RTT 判定与上下限 clamp 均为真实逻辑 | 未开始 |
| N-08 | 数据 `EncodedFrame{frame_id,pts_us,capture_ts_us,is_key,slices:Vec<Bytes>}` | crates/beam-proto/src/lib.rs:80-91；net 侧重导出 crates/beam-net/src/lib.rs:61 | 已实现 | 字段逐项一致（含 `slices: Vec<bytes::Bytes>`） | 未开始 |
| N-09 | 数据 `AssembledFrame` | crates/beam-proto/src/lib.rs:110-121 | 已实现 | frame_id/pts_us/capture_ts_us/is_key/payload 齐全 | 未开始 |
| N-10 | 数据 `Feedback{loss,rtt,recv_rate}` | crates/beam-proto/src/message.rs:52-59 | 已实现 | 字段逐项一致 | 未开始 |
| N-11 | 线程：tokio 多线程运行时 | crates/beam-net/Cargo.toml:20、crates/beam-net/Cargo.toml:24；工作区 tokio features 见 Cargo.toml:45-53 | 已实现 | tokio 启用 `rt-multi-thread`；beam-net 无自己的运行时构造代码（由上层提供） | 未开始 |
| N-12 | 线程：发送路径专用任务,UDP GSO 批量发包 | 现状 crates/beam-net/src/endpoint.rs:253-261；未找到（已搜索：tokio::spawn、sendmmsg、GSO、send_datagrams、UDP GSO、batch） | 缺失 | `send_frame` 是同步函数，逐包调用 `send_datagram`（单包一次系统调用），既无专用发送任务也无 GSO 批量；全仓 `crates/` 下无任何 GSO/sendmmsg 符号 | 已完成 |
| N-13 | 线程：接收重组单任务 | crates/beam-net/src/endpoint.rs:277-288 | 偏差 | 重组逻辑内联在调用方 `next_frame()` future 里，随调用方任务执行；没有独立的接收/重组任务（无 tokio::spawn，见 N-12 搜索集） | 已完成 |
| N-14 | 线程：FEC 解码在 rayon 或 blocking 线程 | crates/beam-net/src/datagram.rs:268-282；自述 crates/beam-net/src/lib.rs:32；未找到（已搜索：rayon、spawn_blocking、blocking、thread::spawn） | 缺失 | `reconstruct_group` 在重组循环内联调用；仓库内无 rayon 依赖，也无 spawn_blocking/专用线程 | 已完成 |
| N-15 | 错误码 2001 连接超时 | crates/beam-net/src/endpoint.rs:113-118；crates/beam-net/src/error.rs:8、44-48 | 已实现 | connect 用 `tokio::time::timeout` 真实产出 `NetError::ConnectTimeout` → 2001 | 未开始 |
| N-16 | 错误码 2002 证书拒绝 | crates/beam-net/src/error.rs:10、51-55；实际产生点 crates/beam-net/src/endpoint.rs:58、64、89、95 | 偏差 | `CertRejected` 只用于「装载本机证书失败 / 构造 QUIC TLS 配置失败」；对端证书被拒时 TLS 握手失败被归为 `NetError::Io`（crates/beam-net/src/endpoint.rs:119-121）→ 上报 E2008，UI 拿不到 E2002 | 已完成 |
| N-17 | 错误码 2003 datagram 不支持 | crates/beam-net/src/endpoint.rs:207-208；crates/beam-net/src/error.rs:12、58-59 | 已实现 | quinn `UnsupportedByPeer` / `Disabled` 均映射到 `DatagramUnsupported` → 2003 | 未开始 |
| N-18 | 错误码 2004 帧过期丢弃 | crates/beam-net/src/error.rs:14、62-66；crates/beam-net/src/datagram.rs:316-328 | 占位 | 变体与码值齐全，但生产代码无处构造：`Reassembler::expire()` 只返回过期帧号数组，doc 注释写「调用方应记为 E2004」而 beam-net 内无此调用方；`NetError::FrameExpired` 全文仅出现在 crates/beam-net/src/error.rs 与其单测（crates/beam-net/src/error.rs:163） | 已完成 |
| N-19 | 配置：`fec_ratio`（默认 0.2…） | crates/beam-net/src/config.rs:19、81 | 已实现 | `DEFAULT_FEC_RATIO = 0.2`，`NetConfig::default().fec_ratio = 0.2`，并有默认值单测 crates/beam-net/src/config.rs:126 | 未开始 |
| N-20 | 配置：`fec_ratio`（…码率>150 Mbps 时下调到 0.1） | 规则函数 crates/beam-net/src/config.rs:109-116；发送路径 crates/beam-net/src/endpoint.rs:255 | 偏差 | `adaptive_fec_ratio` 实现正确且有单测（crates/beam-net/src/config.rs:134-141），但发送路径直接使用 `self.cfg.fec_ratio`，从不调用该函数；全仓 `adaptive_fec_ratio` 仅出现在 crates/beam-net/src/config.rs | 已完成 |
| N-21 | 配置：`max_datagram=1200` | crates/beam-net/src/config.rs:22、82；净负载 crates/beam-net/src/config.rs:98-101 | 已实现 | 默认 1200，扣 16 字节帧头后 1184（§5 负载上限） | 未开始 |
| N-22 | 配置：`jitter_frames`（有线 0,Wi-Fi 自适应 0–1） | 字段 crates/beam-net/src/config.rs:58、83；未找到（已搜索：jitter、JitterBuffer、抖动缓冲、dejitter、jitter_frames 消费点） | 缺失 | 只有配置字段与默认值 0；仓库内无任何抖动缓冲实现（`jitter` 仅命中 crates/beam-perf/src/stage.rs:24 的统计阶段枚举与 crates/beam-config/src/model.rs:61 的配置字段），Wi-Fi 自适应逻辑不存在 | 已完成 |
| N-23 | 配置：`bitrate_min/max` | crates/beam-net/src/config.rs:60-62、84-85 | 已实现 | 默认 2 Mbps / 150 Mbps。注意：`RateController` 需调用方手动传入这两个值（crates/beam-net/src/rate.rs:68），未与 `NetConfig` 自动接线 | 未开始 |
| N-24 | 性能：分包+FEC 编码 T1 ≤0.8 ms、T2 ≤0.4 ms、T3 ≤0.3 ms;重组+FEC 恢复同量级 | 未找到（已搜索：benches 目录、criterion、bench、#[bench]、Instant::now 计时断言） | 缺失 | 全仓没有任何 `benches/` 目录或基准测试依赖；crates/beam-net/Cargo.toml:23-24 只有 tokio dev-dep，无 criterion。T1/T2/T3 预算无任何实测支撑 | 已完成 |
| N-25 | 测试：丢包/乱序/抖动注入(自写 UDP 代理) | 进程内替代用例 crates/beam-net/src/datagram.rs:384-402、426-450；未找到（已搜索：udp proxy、UdpProxy、proxy、注入、抖动 jitter 注入用例） | 缺失 | 丢包/乱序仅以「进程内少投一片/倒序投递」模拟（不经过真实 UDP）；无自写 UDP 代理，无抖动（时延）注入 | 已完成 |
| N-26 | 测试：FEC 恢复率 | crates/beam-net/src/datagram.rs:426-450；crates/beam-net/src/fec.rs:248-264 | 偏差 | 只有「丢 2 片仍能恢复」的点状用例；无按丢包率扫描的恢复率统计/断言，无法量化恢复率 | 已完成 |
| N-27 | 测试：码率控制仿真 | crates/beam-net/src/rate.rs:169-206 | 已实现 | 用合成 `Feedback` 序列驱动控制器，断言降/升/保持与上下限 clamp；仿真真实存在（但 `RateController` 未接入真实网络反馈路径，全仓仅 crates/beam-net/src/rate.rs 出现） | 未开始 |
| N-28 | 里程碑:MVP(无 FEC)/v1(FEC+码率控制+抖动缓冲) | crates/beam-net/Cargo.toml:1-27（无 `[features]`）；FEC 默认开启 crates/beam-net/src/config.rs:81、crates/beam-net/src/endpoint.rs:255 | 偏差 | 无任何 feature 门控：FEC 默认 0.2 恒开（MVP 阶段文档要求「无 FEC」）；码率控制虽已实现但未接线（N-27 注）；抖动缓冲完全缺失（N-22） | 已完成 |
| N-29 | 职责：视频/音频 datagram 分包与重组 | 视频 crates/beam-net/src/datagram.rs:60-142；音频 未找到（已搜索：FrameKind::Audio、audio packetize、音频分包） | 缺失 | `packetize` 只产出 `FrameKind::Video` / `FrameKind::Fec`（crates/beam-net/src/datagram.rs:120-133）；proto 已有 `FrameKind::Audio`（crates/beam-proto/src/frame.rs:35），beam-net 无音频路径（音频本身属 §3.7 的 v1 范围） | 已完成 |
| N-30 | 职责：连接统计 | crates/beam-net/src/endpoint.rs:243-245 | 已实现 | `Conn::stats()` 透出 `quinn::ConnectionStats` | 未开始 |

##### §3.3 beam-pair

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| P-01 | 边界：依赖 proto、rustls、rcgen、spake2;禁止依赖 UI | crates/beam-pair/Cargo.toml:12-25 | 已实现 | 四个依赖齐备；无任何 UI/Tauri/图形依赖 | 未开始 |
| P-02 | 边界：不做网络 IO(PIN 交换消息由 session 经 net 传递) | crates/beam-pair/Cargo.toml:11-25；crates/beam-pair/src/pin.rs:180-230 | 已实现 | 无 quinn/tokio/socket 依赖；所有对外函数同步，PIN 交换只收发字节消息 | 未开始 |
| P-03 | 职责：Ed25519 自签证书(rcgen) | crates/beam-pair/src/identity.rs:120-148 | 已实现 | `PKCS_ED25519` 密钥对 + `params.self_signed` 真生成 Ed25519 自签证书 | 未开始 |
| P-04 | 职责：信任库 | crates/beam-pair/src/trust.rs:72-142 | 已实现 | `TrustStore` 支持 trust.json 载入/保存、按指纹 upsert/remove/query | 未开始 |
| P-05 | 职责：二维码载荷 `beam://pair?h=ip:port&fp=..&t=token` | crates/beam-pair/src/qr.rs:10、27-35、38-87 | 已实现 | 前缀常量 `beam://pair?`，`to_uri` 逐字段输出 h/fp/t，`from_uri` 可解析回来，格式与文档一致 | 未开始 |
| P-06 | 职责：SPAKE2 PIN | crates/beam-pair/src/pin.rs:180-230 | 已实现 | `Spake2::<Ed25519Group>::start_a/start_b/finish` 真跑 SPAKE2，共享密钥 + HMAC 密钥确认 | 未开始 |
| P-07 | 职责：实现 rustls 校验器 | crates/beam-pair/src/verifier.rs:27-72（服务端）、94-142（客户端） | 已实现 | 按文档 §4「TLS 校验回调由 pair 提供」，实现 `ServerCertVerifier` 与 `ClientCertVerifier` | 未开始 |
| P-08 | 接口：`Identity::load_or_create(store)` | crates/beam-pair/src/identity.rs:108-117 | 已实现 | 签名一致；先 load 再 generate+save，`KeyStore` 为可注入后端 | 未开始 |
| P-09 | 接口：`fn qr_payload(&self, addr) -> PairPayload` | crates/beam-pair/src/identity.rs:166-173 | 已实现 | 签名一致；填入本机指纹与一次性 token | 未开始 |
| P-10 | 接口：`PinPairing::start(pin) -> (Self, Msg)` | crates/beam-pair/src/pin.rs:180-187 | 已实现 | 返回 `(PinPairing, Vec<u8>)`（Msg = SPAKE2 首条消息字节）。注意：首条消息需再经 `client_challenge` 包装成 postcard `PairChallenge` 才能被对端 `respond`/`finish` 消费（crates/beam-pair/src/pin.rs:241-248），接口两侧不对称 | 未开始 |
| P-11 | 接口：`PinPairing::finish(msg) -> Result<Fingerprint,_>` | crates/beam-pair/src/pin.rs:213-230 | 偏差 | 返回 `Result<PinOutcome, PairError>`，`PinOutcome` 为 `{fingerprint, shared_key}`（crates/beam-pair/src/pin.rs:45-50），不是文档写的 `Fingerprint` 直接返回 | 未开始 |
| P-12 | 接口：`impl PeerVerifier for TrustStore` | crates/beam-pair/src/trust.rs:144-148；trait 定义 crates/beam-pair/src/fingerprint.rs:82-85 | 已实现 | 实现体为真实查库 `is_trusted`，非空实现 | 未开始 |
| P-13 | 数据：`Fingerprint([u8;32])` | crates/beam-pair/src/fingerprint.rs:12-13 | 已实现 | 定长 32 字节元组，等价 SHA-256(cert DER)（crates/beam-pair/src/fingerprint.rs:25-30） | 未开始 |
| P-14 | 数据：`TrustedPeer{fp,name,platform,paired_at,perms}` | crates/beam-pair/src/trust.rs:39-50 | 已实现 | 五个字段逐项一致（`perms` 为 `PeerPerms`，crates/beam-pair/src/trust.rs:15-24） | 未开始 |
| P-15 | 错误码 3001 PIN 错误 | crates/beam-pair/src/error.rs:9、36-37；产生点 crates/beam-pair/src/pin.rs:202、219、222 | 已实现 | SPAKE2 失败与密钥确认不一致均映射 `PinIncorrect` → 3001 | 未开始 |
| P-16 | 错误码 3002 次数超限锁定 60 s | crates/beam-pair/src/error.rs:10、39-46；crates/beam-pair/src/pin.rs:148-157、71 | 已实现 | `AttemptGuard::record_failure` 超 `max_attempts` 锁定 `lockout`（默认 60 s）并返回 3002 | 未开始 |
| P-17 | 错误码 3003 token 过期 | crates/beam-pair/src/error.rs:12、48-50；crates/beam-pair/src/pin.rs:100-105 | 已实现 | `PairingToken::verify` 按 TTL 判定并返回 `TokenExpired` → 3003 | 未开始 |
| P-18 | 错误码 3004 指纹不匹配 | crates/beam-pair/src/error.rs:15、52-54 | 偏差 | 变体与码值齐全，但生产代码从不构造 `PairError::FingerprintMismatch`；实际路径在 crates/beam-pair/src/verifier.rs:40-44 与 110-114 —— 以字符串 `"E3004: ..."` 塞进 rustls `TlsError::General`，上层无法按 code 结构化处理 | 未开始 |
| P-19 | 配置：`pin_len=4`、`pin_ttl=120s`、`max_attempts=3` | crates/beam-pair/src/pin.rs:54-74 | 已实现 | `PairingPolicy::default()` 三项与文档一致，并有单测 crates/beam-pair/src/pin.rs:272-279 | 未开始 |
| P-20 | 线程：同步 API,在调用方任务内执行 | crates/beam-pair/src/pin.rs:180、213、261；crates/beam-pair/src/trust.rs:84-111 | 已实现 | 无 async/await，无自建线程 | 未开始 |
| P-21 | 测试：中间人模拟 | crates/beam-pair/src/verifier.rs:157-166 | 偏差 | 仅「未知指纹证书被校验器拒绝」的验证器级单测；没有握手级中间人模拟（不构造被劫持的连接/证书链去攻击真实握手流程） | 未开始 |
| P-22 | 测试：错误 PIN | crates/beam-pair/src/pin.rs:299-314 | 已实现 | 双方用不同 PIN 时密钥确认失败 → 断言 3001 | 未开始 |
| P-23 | 测试：过期 token | crates/beam-pair/src/pin.rs:330-338 | 已实现 | 断言 3003 | 未开始 |
| P-24 | 里程碑:MVP | crates/beam-pair/Cargo.toml:1-29（无 `[features]`） | 已实现 | 无 feature 门控，职责内功能全部可用 | 未开始 |

##### §3.4 beam-discovery

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| D-01 | 职责：mDNS 广播 `_beam._udp.local` | crates/beam-discovery/src/records.rs:11 | 已实现 | `SERVICE_TYPE = "_beam._udp.local"`，注册时直接使用（crates/beam-discovery/src/mdns.rs:44-51） | 已完成 |
| D-02 | 职责：TXT 字段 id/name/ver/fp(前 8 位)/port | crates/beam-discovery/src/records.rs:14-25、46-58 | 已实现 | 五个字段名与文档一致；`fp` 广播时按字符截前 8 位（crates/beam-discovery/src/records.rs:52-55），有单测 crates/beam-discovery/src/records.rs:133-150 | 未开始 |
| D-03 | 边界：只发现不连接 | crates/beam-discovery/Cargo.toml:11-17；crates/beam-discovery/src/mdns.rs:1-187 | 已实现 | 无 quinn/socket/连接代码，本 crate 只广播与浏览 | 未开始 |
| D-04 | 边界：依赖 mdns-sd | crates/beam-discovery/Cargo.toml:13 | 已实现 | 依赖存在并真实使用（crates/beam-discovery/src/mdns.rs:12） | 未开始 |
| D-05 | 接口：`Advertiser::start(info)` | crates/beam-discovery/src/mdns.rs:33-64 | 已实现 | 真建 `ServiceDaemon`、注册 `ServiceInfo`、返回可 unregister 的实例 | 未开始 |
| D-06 | 接口：`Browser::start() -> impl Stream<Item=DiscoveryEvent>` | crates/beam-discovery/src/mdns.rs:109-141；`impl Stream` crates/beam-discovery/src/mdns.rs:149-155 | 已实现 | `Browser::start()` 返回 `Result<Browser, DiscoveryError>`，且 `Browser: Stream<Item = DiscoveryEvent>`（多一层 Result，语义等价）；浏览事件经独立线程 + 无界通道转成 Stream（crates/beam-discovery/src/mdns.rs:118-135） | 已完成 |
| D-07 | 配置：`enabled` | 键位于 crates/beam-config/src/model.rs:67、264、321；未找到（已搜索：beam-discovery 内的 enabled / discovery_enabled 消费点） | 偏差 | 配置模型有 `discovery_enabled`，但 beam-discovery 自身没有配置入口，`Advertiser::start` / `Browser::start` 均无开关，上层也无法通过该 crate 关掉发现 | 未开始 |
| D-08 | 配置：`iface_allowlist` | 键位于 crates/beam-config/src/model.rs:69、266、322；未找到（已搜索：iface_allowlist / allowlist 在 beam-discovery 内消费点） | 偏差 | 配置模型有网卡白名单，但 beam-discovery 的 `Advertiser::start` / `Browser::start` 不接收该参数，也没有按网卡过滤的逻辑 → 白名单当前无效 | 未开始 |
| D-09 | 错误码 `E4xxx` | crates/beam-discovery/src/error.rs:4-13、31-59 | 已实现 | 4001 守护进程启动失败 / 4002 注册失败 / 4003 浏览失败 / 4004 广播信息非法，均为真实产生点（crates/beam-discovery/src/mdns.rs:35、39、52、59、110、115） | 未开始 |
| D-10 | 测试：多网卡 | 未找到（已搜索：iface、网卡、多网卡、allowlist、interface 相关测试） | 缺失 | 无任何多网卡场景用例；相关逻辑（网卡白名单）本身也未接线（D-08） | 未开始 |
| D-11 | 测试：组播被阻断回退手动 IP | crates/beam-discovery/src/mdns.rs:231-236；未找到（已搜索：手动、manual、fallback、回退手动 IP） | 缺失 | 唯一浏览用例是「能启动就停」的宽容断言（失败即跳过）；手动 IP 回退逻辑未实现（crates/beam-discovery/src/lib.rs:36 自述「回退逻辑在 host / client 层」，但 host / client 侧亦无对应代码） | 未开始 |
| D-12 | 里程碑:MVP | crates/beam-discovery/Cargo.toml:1-21（无 `[features]`） | 已实现 | 无 feature 门控 | 未开始 |

#### 文档未覆盖但代码里有的

1. **beam-discovery 声明了 `beam-proto` 但从不使用**（crates/beam-discovery/Cargo.toml:12）。全 crate 无任何 `beam_proto::` 引用 —— 无用依赖，违反「只依赖 mdns-sd」的最小依赖意图。
2. **beam-net 扩展了 E2005–E2008**（crates/beam-net/src/error.rs:15-20、107）。文档只列 2001–2004；扩展本身合理，但正因多出 E2008「网络 IO 失败」，对端证书被拒被归入 IO 错误，掩盖了 E2002（N-16）。
3. **beam-pair 扩展了 E3005–E3007**（crates/beam-pair/src/error.rs:56-72）：SPAKE2 消息非法 / 存储失败 / 载荷非法。文档未列。
4. **数据报负载 4 字节大端长度前缀是实现约定，文档未定义**（crates/beam-net/src/datagram.rs:33-34、88）。发送端与接收端自洽，但任何按文档另写的实现（如 §7 的 gamestream 兼容层）都无法从文档得知该约定，存在互操作风险。
5. **切片边界标记在真实发送路径永不置位**：`packetize` 接受 `slice_boundary` 参数（crates/beam-net/src/datagram.rs:64、113-115），但 `Conn::send_frame` 恒传 `false`（crates/beam-net/src/endpoint.rs:255），§5 帧头 flags bit2 在线上永远为 0。
6. **控制 / 输入流的打开接口在 §3.2 未列出但已存在**：`Conn::open_bi/open_uni/accept_bi`（crates/beam-net/src/endpoint.rs:175-200）—— 与 §5 的「控制 stream 0 + 输入单向流」一致，属合理扩展。
7. **`NetConfig` 增加了文档未列的字段**：`connect_timeout`、`max_idle_timeout`、`server_name`、`listen_addr`、`bind_addr`、`identity`（crates/beam-net/src/config.rs:64-74）。其中 `identity` 缺失即报 E2007，是实现必需，但属于文档未规定的扩展面。

#### 最该优先补的（最多 3 条）

1. **§3.2 线程模型整体未落地**：发送路径没有专用任务也没有 UDP GSO 批量发包（N-12），接收重组内联在调用方 future（N-13），FEC 解码留在接收任务内（N-14），而这三项正是 T1/T2/T3 延迟预算的载体；叠加完全没有基准测试（N-24），当前「能跑通」并不等于「达标」。
2. **v1 的抖动缓冲完全缺失，MVP 的 FEC 反而默认开启**：jitter_frames 只有配置字段没有任何缓冲实现（N-22），Wi-Fi 场景没有抖动吸收；FEC 则无 feature 门控、默认 0.2 恒开（N-28），在文档定义的 MVP 阶段白占带宽。
3. **三个文档错误码没有生产路径**：E2002 对端证书拒绝现在报 E2008（N-16）、E2004 帧过期从不被产生（N-18）、E3004 只是塞进 rustls 错误的字符串（P-18）。§3.11 要求「UI 只根据 code 显示文案」，这三条会让 UI 拿到错误或拿不到的 code。
### 6. 平台层 crate（capture/encode/audio/input/decode/render）（§3.5 / §3.6 / §3.7 / §3.8 / §3.10）

> 来自 `part-3.md`

#### 条目

##### §3.5 beam-capture

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| C-01 | 「WGC(主)/DXGI 复制(回退)抓屏,输出 GPU 上的 D3D11 纹理」（WGC 主路径） | `crates/beam-capture/src/source.rs:115-149` | 占位 | `WgcCaptureSource::start()` 在 `wgc` feature 下只做 `let _ = &cfg;` 后 `Ok(())`（`source.rs:120-123`），不枚举显示器、不创建设备、不建会话；`latest()` 在两个分支都返回 `None`（`source.rs:137,141`）。`windows_capture` 在 `Cargo.toml:27` 声明为 optional，但 `src/` 中从未被引用。**默认 feature 下 `start` 返回 `E5004`**（`source.rs:125-129`）。 | 已完成 |
| C-02 | 「DXGI 复制(回退)抓屏」（DXGI 回退路径） | 未找到（已搜索：`Dxgi`、`DXGI`、`dxgi`、`windows-capture`；`crates/beam-capture/src/` 下仅 `source.rs:32` 的枚举变体与 `lib.rs:7,29,39` 的文档注释命中） | 缺失 | `CaptureBackend::Dxgi` 只是一个枚举值（`source.rs:30-32`），**没有任何 DXGI 桌面复制实现**，也没有 `DxgiCaptureSource` 之类的类型。文档标注该能力属于 v1（§3.5 第 124 行「里程碑:MVP(WGC)/v1(DXGI 回退、虚拟显示器)」），故属里程碑内缺失。 | 已完成 |
| C-03 | 「设置 `MinUpdateInterval`」 | `crates/beam-capture/src/source.rs:48-51` | 占位 | 只有配置字段 `pub min_update_interval: Option<u32>`。代码中没有任何一处把它传给 WGC 的帧池/会话 API（`windows-capture` 未被引用）。字段注释（`source.rs:49-50`）复述了文档的 SDK 26100 限制，但无实现。 | 已完成 |
| C-04 | 「光标单独通道」 | 未找到（已搜索：`cursor`、`Cursor`、`光标`；`crates/` 全目录零命中） | 缺失 | 没有任何光标通道、没有 `cursor` 类型或字段。`CapturedFrame`（`source.rs:69-76`）也无光标相关成员。 | 已完成 |
| C-05 | 「边界:不做颜色转换和编码」 | `crates/beam-capture/Cargo.toml:16-27` | 已实现 | beam-capture 的依赖只有 `beam-proto` / `thiserror` / `tracing` + windows 平台 crate，源码中无任何颜色转换或编码代码（`crates/beam-capture/src/` 三个文件均无 YUV/NV12/编码调用）。边界约束成立。 | 未开始 |
| C-06 | 「边界:不拷到 CPU」 | `crates/beam-capture/src/source.rs:8-9,69-76` | 已实现 | `CapturedFrame.tex` 是 `Texture`（Windows 上即 `ID3D11Texture2D` 的类型别名，`source.rs:9`），全链路无 CPU 拷贝路径（无 `Vec<u8>` / `Bytes` 成员）。 | 未开始 |
| C-07 | 「禁止依赖 net」 | `crates/beam-capture/Cargo.toml:16-27` | 已实现 | `Cargo.toml` 中无 `beam-net`（或任何 net 类 crate）依赖；`src/` 中也无 net 相关 import。 | 未开始 |
| C-08 | 「`pub trait CaptureSource: Send { fn start(&mut self, cfg: CaptureConfig) -> Result<(), CapError>; fn latest(&mut self) -> Option<CapturedFrame>; fn stop(&mut self); }`」 | `crates/beam-capture/src/source.rs:79-86` | 已实现 | 逐字一致：`Send` 约束在 `:79`，`start` `:81`、`latest` `:83`、`stop` `:85`，签名与返回类型完全吻合。 | 未开始 |
| C-09 | 「`pub struct CapturedFrame { pub tex: ID3D11Texture2D, pub ts_us: u64, pub dirty: bool }`」 | `crates/beam-capture/src/source.rs:69-76`（类型别名 `:9`） | 已实现 | 三个字段名与类型逐字吻合；Windows 上 `Texture` 即 `ID3D11Texture2D`（`source.rs:9`）。注：非 Windows 平台是占位 struct（`source.rs:15-22`）；且 `dirty` 字段目前**无任何生产者**（没有任何实现会给它赋 `true`）。 | 未开始 |
| C-10 | 「线程:WGC 回调线程只更新"最新帧"槽(单槽交换)」 | 未找到（已搜索：`thread::spawn`、`std::thread::`、`sync_channel`、`单槽`、`slot`；`crates/beam-capture/` 下零命中） | 缺失 | 既无 WGC 帧回调，也无单槽交换结构（无 `ArcSwap` / `UnsafeCell<Option<..>>` 之类的槽位）。`WgcCaptureSource` 的字段只有 `cfg` 与 `stopped`（`source.rs:93-96`），没有任何帧存储。 | 已完成 |
| C-11 | 「编码节拍线程按目标帧率拉取」 | `crates/beam-capture/src/source.rs:52-53` | 占位 | `CaptureConfig.target_fps` 字段存在（`source.rs:53`，默认 60），但**没有任何节拍线程**去按它拉取（见 C-10 的搜索结果）。 | 已完成 |
| C-12 | 「错误码 `E5xxx`(5001 无显示器)」 | `crates/beam-capture/src/error.rs:6,33-34` | 占位 | 错误码常量 `NO_DISPLAY = 5001` 与 `CapError::NoDisplay` 变体已定义（并有文案映射 `error.rs:18`），但**没有任何代码路径会产生它**——`start()` 只会返回 `E5004`。 | 已完成 |
| C-13 | 「5002 独占全屏无法抓取→切 DXGI」 | `crates/beam-capture/src/error.rs:8,37-38` | 占位 | 变体与文案（`error.rs:19`）齐备，但无产生路径；且「→切 DXGI」所依赖的 DXGI 后端本身缺失（见 C-02），该错误码即便触发也无回退目标。 | 已完成 |
| C-14 | 「5003 设备丢失」 | `crates/beam-capture/src/error.rs:10,41-42` | 占位 | 变体与文案（`error.rs:20`）齐备，无产生路径；无 D3D11 设备就无从检测设备丢失。 | 已完成 |
| C-15 | 「配置:`backend=auto\|wgc\|dxgi`」 | `crates/beam-capture/src/source.rs:26-33,45`；配置模型处 `crates/beam-config/src/model.rs` 中未找到（已搜索 `backend`） | 偏差 | crate 内 `CaptureBackend` 三值枚举**已实现**且 `Auto` 为默认（`source.rs:26-39,57`）；但**全局配置模型 `beam-config` 里没有 `backend` 项**——`crates/beam-config/src/model.rs` 全文无 `backend` 字段（对比同文件 `encoder`/`codec` 有，`model.rs:494,499`），因此该配置项**无法从配置文件/命令行/环境变量落到运行时**。 | 已完成 |
| C-16 | 「配置:`display_id`」 | `crates/beam-capture/src/source.rs:46-47`；`crates/beam-config/src/model.rs` 中未找到（已搜索 `display_id`） | 偏差 | crate 内 `CaptureConfig.display_id: Option<u32>` 存在（默认 `None` = 主显示器，`source.rs:47,59`），但 beam-config 配置模型中无 `display_id` 项，用户无法配置目标显示器。 | 已完成 |
| C-17 | 「配置:`min_update_interval`」 | `crates/beam-capture/src/source.rs:48-51`；`crates/beam-config/src/model.rs` 中未找到（已搜索 `min_update_interval`） | 偏差 | 同上：crate 内字段存在，但 beam-config 无对应配置项，且无实现（见 C-03）。 | 已完成 |
| C-18 | 「性能:抓屏+取纹理 T1 ≤1.5 ms、T2/T3 ≤1 ms」 | 未找到（已搜索：`bench`、`criterion`、`\[\[bench\]\]`、`elapsed`、`Instant`；全仓无任何基准框架） | 缺失 | 全仓**没有引入 criterion，也没有 `benches/` 目录**（已搜索 `criterion` / `[[bench]]` / `benches/`，零命中）。§3.6 / §3.10 要求的「P99 编码耗时基准」「各显卡解码基准」同样无落点。 | 已阻塞 |
| C-19 | 「必须能交付 ≥ 目标帧率(主机显示器刷新率需 ≥ 目标)」 | 未找到（已搜索：`refresh`、`display`、`monitor`、`EnumDisplayDevices`；`crates/beam-capture/src/` 仅 `source.rs:46-47` 的 `display_id` 命中） | 缺失 | 无显示器枚举代码（`windows` 依赖里也未启用 `Win32_Graphics_DirectX` / 显示枚举相关 feature，`crates/beam-capture/Cargo.toml:22-26`），无显示器刷新率查询，因此无法校验「刷新率 ≥ 目标帧率」。 | 已阻塞 |
| C-20 | 「测试:帧率计数(165/240Hz 显示器)」 | `crates/beam-capture/src/source.rs:151-187`（测试模块仅 3 个用例） | 缺失 | 该 crate 的全部单测为 `default_backend_is_auto`、`start_without_backend_feature_reports_e5004`、`stop_resets_state`（`source.rs:155-187`），**没有任何帧率计数测试**，也没有 165/240Hz 显示器相关代码。 | 已阻塞 |
| C-21 | 「测试:设备丢失恢复」 | 未找到（已搜索：`DeviceLost`、`recover`、`reset`、`recreate`；`crates/beam-capture/src/` 仅 `error.rs:10,41-42,53-55` 的错误码定义命中） | 缺失 | `CapError::DeviceLost` 只有定义没有产生者，也没有设备重建/恢复逻辑。 | 已阻塞 |
| C-22 | 「测试:多显示器」 | 未找到（已搜索：`EnumDisplay`、`monitor`、`display_id`；`crates/beam-capture/src/` 仅 `source.rs:47` 的配置字段命中） | 缺失 | 无多显示器枚举/选择逻辑。 | 已阻塞 |
| C-23 | 「测试:Windows 10 22H2 实测抓屏帧率上限」 | 未找到（已搜索：`win10`、`22H2`、`fps_limit`；`crates/beam-capture/` 零命中） | 缺失 | 无任何 Win10 22H2 专项代码或实测脚手架；文档还特别强调「M1 验收须含 Windows 10 实测项」（§3.5 第 125 行），该验收项在仓库中无对应物。 | 已阻塞 |
| C-24 | 「平台限制:`MinUpdateInterval` 需 SDK 26100(Windows 11 24H2)」 | `crates/beam-capture/src/source.rs:49-50`（仅注释）；`crates/beam-capture/Cargo.toml:22-26` | 缺失 | 代码只在注释里复述了这条限制（`source.rs:49-50`、`lib.rs:38`），**没有任何 SDK 版本判定**（`windows` 依赖也未启用任何版本查询 API）。加上 C-03（无实现），这条限制目前既未被实现也未被强制。 | 已完成 |
| C-25 | 「Windows 10 22H2 无此能力,MVP 阶段又无 DXGI 回退,抓屏上限约 60fps」 | 未找到（已搜索：同上 C-23 全部符号） | 缺失 | 无对应的能力降级、无帧率上限兜底逻辑（`CaptureConfig.target_fps` 也无上限约束），也无实测数据。 | 已完成 |

##### §3.6 beam-encode

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| E-01 | 「GPU 上 RGB→NV12(D3D11 VideoProcessor 或 compute shader)」 | `crates/beam-encode/src/encoder.rs:200-205` | 占位 | 只有一行注释「真实实现：D3D11 硬件帧上下文 → D3D11 VideoProcessor 颜色转换 → NVENC/AMF/QSV」（`encoder.rs:203`），实际代码 `let _ = f;` 后返回 `Err(EncError::BackendDisabled)`。无 VideoProcessor、无 compute shader、无 NV12 格式代码。 | 已完成 |
| E-02 | 「NVENC/AMF/QSV 硬编(ffmpeg-next 封装,D3D11 硬件帧上下文)」 | `crates/beam-encode/src/encoder.rs:198-211`；`crates/beam-encode/Cargo.toml:30` | 占位 | `ffmpeg-next` 声明为 optional 依赖，但 `src/` 中**从未被引用**（已搜索 `ffmpeg_next` / `ffmpeg::`，`crates/` 全目录零命中）。`EncoderKind` 三个变体（`encoder.rs:45-54`）只用于名字解析（`encoder.rs:74-82`），不驱动任何编码器选择。 | 已完成 |
| E-03 | 「低延迟参数」 | `crates/beam-encode/src/encoder.rs:98-99` | 占位 | `EncoderCaps.low_latency: bool` 字段存在（注释还写了「ultra-low-latency / vbr_latency / veryfast」），但 `probe()` 恒返回空列表（`encoder.rs:161-171`），该字段**没有任何生产者**，也没有任何低延迟开关传给编码器。 | 已完成 |
| E-04 | 「切片输出」 | `crates/beam-encode/src/encoder.rs:117-118`；`crates/beam-proto/src/lib.rs:89-90` | 占位 | 数据结构齐备：`EncodeConfig.slices: u32`（`encoder.rs:118`）+ `EncodedFrame.slices: Vec<Bytes>`（`beam-proto/src/lib.rs:90`），但 `encode()` 从不产出切片（恒报错）。文档标注切片属 v1（§3.6 第 135 行「里程碑:MVP / v1(切片、SFE)」）。 | 已完成 |
| E-05 | 「强制 IDR」 | `crates/beam-encode/src/encoder.rs:213-215` | 占位 | `request_idr()` 只把 `self.idr_requested = true`（`encoder.rs:214`），没有向任何编码器上下文下发 IDR 指令；该标志也无人读取。单测 `bitrate_and_idr_are_recorded`（`encoder.rs:272-278`）只断言标志位为真。 | 已完成 |
| E-06 | 「码率热更新」 | `crates/beam-encode/src/encoder.rs:217-222` | 占位 | `set_bitrate()` 只改内存字段 `self.bitrate_bps` 并同步到 `cfg`（`encoder.rs:218-221`），没有调用任何编码器的码率更新 API（无 ffmpeg 上下文）。文档要求「码率热更新」且测试要求「码率切换无花屏」（见 E-26），两者当前都无实现。 | 已完成 |
| E-07 | 「边界:不负责发包」 | `crates/beam-encode/Cargo.toml:19-30` | 已实现 | crate 无任何 net / 发包代码；返回值是协议层 `EncodedFrame`（`encoder.rs:142`），发包由 beam-net 承担。 | 未开始 |
| E-08 | 「边界:禁止依赖 net/session」 | `crates/beam-encode/Cargo.toml:19-30` | 已实现 | `Cargo.toml` 依赖仅 `beam-proto` / `thiserror` / `tracing`（+ optional `ffmpeg-next`），无 `beam-net`、无 `beam-session`。`encoder.rs:3-8` 还专门注释说明为何不能依赖 beam-capture。 | 未开始 |
| E-09 | 「`trait VideoEncoder { fn encode(&mut self, f:&CapturedFrame) -> Result<EncodedFrame, EncError>; fn request_idr(&mut self); fn set_bitrate(&mut self,bps:u32); }`」 | `crates/beam-encode/src/encoder.rs:140-147` | 已实现 | 逐字一致：`encode` `:142`、`request_idr` `:144`、`set_bitrate` `:146`。注：`CapturedFrame` 是 beam-encode 自定义的同形结构（`encoder.rs:30-41`），因 §4 分层禁止 L1 横向依赖——此为有据的等价实现。 | 未开始 |
| E-10 | 「`fn probe() -> Vec<EncoderCaps>`」 | `crates/beam-encode/src/encoder.rs:161-171` | 占位 | 函数签名与返回类型吻合，但**两个 feature 分支都返回 `Vec::new()`**（`encoder.rs:165` 与 `encoder.rs:169`）。开启 `ffmpeg` feature 也一样是空列表——`ffmpeg-next` 从未被调用。 | 已完成 |
| E-11 | 「参数基线:NVENC P1 + ultra-low-latency、CBR、无 B 帧、无 lookahead、VBV=1 帧、intra-refresh」 | 未找到（已搜索：`P1`、`ultra`、`low_latency`、`CBR`、`cbr`、`lookahead`、`vbv`、`intra`、`b_frames`、`preset`；`crates/beam-encode/src/` 零命中） | 缺失 | 整个参数基线在代码中**没有任何对应物**。`EncoderCaps` 里只有 `low_latency` 一个布尔标记（`encoder.rs:99`），无 preset、无 rate-control、无 B 帧/lookahead/VBV/intra-refresh 设置。 | 已完成 |
| E-12 | 「Ada 上 T1 及高帧率档开启 SFE」 | `crates/beam-encode/src/encoder.rs:102-103,119-120` | 占位 | `EncoderCaps.sfe: bool` 与 `EncodeConfig.sfe: bool` 两个字段存在（注释写「split-frame encoding，Ada 起」），但没有任何「按显卡代际 / 按档位自动开启 SFE」的判定逻辑，也没有把 `sfe` 传给编码器。 | 已完成 |
| E-13 | 「参数基线:AMF `ultralowlatency`+`vbr_latency`」 | 未找到（已搜索：`ultralowlatency`、`vbr_latency`、`amf`；`crates/beam-encode/src/` 仅 `encoder.rs:50` 的枚举变体 `Amf` 命中） | 缺失 | AMF 仅是一个枚举名（`encoder.rs:50`）与字符串解析（`encoder.rs:69`），无任何 AMF 参数设置。 | 已完成 |
| E-14 | 「参数基线:QSV `veryfast`」 | 未找到（已搜索：`veryfast`、`qsv`；`crates/beam-encode/src/` 仅 `encoder.rs:52` 的枚举变体 `Qsv` 命中） | 缺失 | 同上，QSV 无参数设置代码。 | 已完成 |
| E-15 | 「线程:单独编码线程(与节拍线程合并),同步调用」 | 未找到（已搜索：`thread::spawn`、`std::thread::`；`crates/beam-encode/` 零命中） | 缺失 | 无任何线程创建。`VideoEncoder::encode` 是同步方法（`encoder.rs:142`），但「单独编码线程」这一线程模型未落地，也没有任何调用方去开线程。 | 已完成 |
| E-16 | 「错误码 `E6xxx`(6001 无可用硬编)」 | `crates/beam-encode/src/error.rs:6,33-34` | 占位 | 变体 + 文案（`error.rs:18`）齐备，但无产生路径：无论 feature 开或关，`encode()` 返回的都是 `E6004`（`encoder.rs:204,209`），而不是 `E6001`。`encoder.rs:159-160` 的注释还声称空 probe「对应 E6001」，实际返回的是 `E6004`——文档注释与代码语义不符。 | 已完成 |
| E-17 | 「6002 会话数超限」 | `crates/beam-encode/src/error.rs:8,37-38` | 占位 | 变体 + 文案（`error.rs:19`）齐备，无产生路径（无编码会话计数概念）。 | 已完成 |
| E-18 | 「6003 参数不支持→降档」 | `crates/beam-encode/src/error.rs:10,41-45` | 占位 | 变体 + 文案（`error.rs:20`）齐备，无产生路径（无参数下发，因而无从判定参数不被支持）。 | 已完成 |
| E-19 | 「配置:`encoder=auto\|nvenc\|amf\|qsv`」 | `crates/beam-encode/src/encoder.rs:45-54,64-82`；`crates/beam-config/src/model.rs:48,499-503` | 已实现 | crate 内 `EncoderKind` 四值枚举 + `as_str`/`from_name` 双向映射齐备（`encoder.rs:64-82`）；全局配置模型有 `video.encoder: Option<String>`（`model.rs:48`）、CLI 覆盖 `encoder`（`model.rs:137`）、环境变量 `BEAM_ENCODER`（`model.rs:176-178`）与取值校验（`model.rs:499-503`）。**取值链路已通**，但下游无编码器可选（见 E-02）。 | 已完成 |
| E-20 | 「配置:`codec=hevc\|h264\|av1`」 | `crates/beam-encode/src/encoder.rs:150-155`；`crates/beam-config/src/model.rs:46,494-498` | 已实现 | `CodecKind` 三值 + `supports_codec()`（AV1 由 `av1` feature 门控，`encoder.rs:152`，与 §7 一致）；配置侧有 `video.codec`（`model.rs:46`）、CLI（`model.rs:136`）、环境变量（`model.rs:173-175`）与校验（`model.rs:494-498`）。 | 已完成 |
| E-21 | 「配置:`slices`」 | `crates/beam-encode/src/encoder.rs:117-118`；`crates/beam-config/src/model.rs` 中未找到（已搜索 `slices`） | 偏差 | crate 内 `EncodeConfig.slices: u32` 存在（默认 0），但**全局配置模型没有 `slices` 项**（`crates/beam-config/src/model.rs` 全文无 `slices`），用户无法配置切片数。且即使配置了也无实现（见 E-04）。 | 已完成 |
| E-22 | 「配置:`sfe`」 | `crates/beam-encode/src/encoder.rs:119-120`；`crates/beam-config/src/model.rs` 中未找到（已搜索 `sfe`） | 偏差 | 同 E-21：crate 内 `EncodeConfig.sfe: bool` 存在，beam-config 无 `sfe` 配置项，无法由用户开启；且无任何「Ada + T1 自动开启」逻辑（见 E-12）。 | 已完成 |
| E-23 | 「性能:颜色转换+编码 T1 ≤7 ms(Ampere 级,Ada ≤4 ms)、T2 ≤3 ms、T3 ≤2 ms」 | 未找到（已搜索：`bench`、`criterion`、`p99`、`P99`；全仓零命中） | 缺失 | 无任何基准设施（同 C-18），无可对照的实测数据。 | 已阻塞 |
| E-24 | 「测试:probe 矩阵」 | `crates/beam-encode/src/encoder.rs:250-256`（单测 `probe_without_backend_returns_empty`） | 缺失 | 唯一相关单测断言的是「probe 返回空」，属于 `占位` 的佐证；**没有覆盖 NVENC/AMF/QSV × HEVC/H.264/AV1 的能力矩阵**。 | 已完成 |
| E-25 | 「测试:各档 P99 编码耗时基准」 | 未找到（已搜索：`criterion`、`bench`、`p99`；全仓零命中） | 缺失 | 同 E-23，无基准框架。 | 已阻塞 |
| E-26 | 「测试:码率切换无花屏」 | 未找到（已搜索：`set_bitrate`、`idr`、`switch`、`artifact`；`crates/beam-encode/src/` 仅 `encoder.rs:217` 命中） | 缺失 | `set_bitrate` 无实现（E-06），因此也不存在验证「切换无花屏」的测试。 | 已阻塞 |

##### §3.7 beam-audio

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| A-01 | 「主机 WASAPI loopback 48 kHz → Opus 5 ms 帧」（WASAPI loopback 采集） | `crates/beam-audio/src/audio.rs:111-152` | 占位 | `WasapiLoopbackSource` 只存 `cfg`/`running` 两个字段（`audio.rs:111-114`）。`start()` 在默认 feature 下返回 `E7004`（`audio.rs:137-141`）；开了 `opus` feature 的分支也只是置标志后 `Ok(())`（`audio.rs:130-136`）。**全 crate 没有任何 WASAPI 调用**（已搜索 `Wasapi`/`WASAPI`/`loopback`，仅命中注释与类型名；`windows` 依赖虽启用了 `Win32_Media_Audio`/`Win32_Media_KernelStreaming`（`crates/beam-audio/Cargo.toml:23-26`）但未被使用）。`next_frame()` 恒返回 `None`（`audio.rs:144-146`）。 | 已完成 |
| A-02 | 「Opus 5 ms 帧」（Opus 编码） | `crates/beam-audio/src/audio.rs:16-20,167-174` | 占位 | 帧长常量正确（`FRAME_DURATION_MS = 5`，`audio.rs:17`；`SAMPLES_PER_FRAME = 240`，`audio.rs:20`），但 `OpusEncoderImpl::encode()` 恒返回 `Err(AudioError::BackendDisabled)`（`audio.rs:170`），且**无论 `opus` feature 开或关都如此**。`opus` 依赖（`Cargo.toml:28`）在 `src/` 中从未被引用。 | 已完成 |
| A-03 | 「Windows 客户端 Opus 解码 + WASAPI 输出」（Opus 解码部分） | `crates/beam-audio/src/audio.rs:189-196` | 占位 | `OpusDecoderImpl::decode()` 恒返回 `Err(AudioError::BackendDisabled)`（`audio.rs:192`），feature 开或关都一样。无任何真实解码。 | 已完成 |
| A-04 | 「Windows 客户端 Opus 解码 + WASAPI 输出」（WASAPI 输出部分） | `crates/beam-audio/src/audio.rs:104-107` | 缺失 | 只有 `trait AudioOutput { fn play(&mut self, pcm:&PcmFrame) -> Result<(), AudioError>; }` 的**接口声明**，**没有任何实现**（已搜索 `impl AudioOutput`：`crates/beam-audio/src/` 零命中；`impl AudioSource` / `impl OpusEncoder` / `impl OpusDecoder` 均有实现，唯独 `AudioOutput` 没有）。也没有 `WasapiAudioOutput` 之类的类型。 | 已完成 |
| A-05 | 「Android 由 core-audio 解码」 | `android/core-audio/src/main/java/dev/beam/core/audio/LowLatencyAudioPlayer.kt:136-148,150-178` | 已实现 | Android 侧是**真实实现**：`MediaCodec.createDecoderByType("audio/opus")` 解码 → `AudioTrack` 低延迟输出（`LowLatencyAudioPlayer.kt:136-148`、`:150-178`），并有 `PcmRingBuffer.kt` 与单测 `src/test/java/dev/beam/core/audio/PcmRingBufferTest.kt`。注：这是 Kotlin 侧实现，超出本次 6 个 Rust crate 的审计范围，在此仅作 §3.7 要求的对照。 | 未开始 |
| A-06 | 「(可复用 ffi 中的 Opus 解码)」 | 未找到（已搜索：`opus`、`Opus`；`crates/beam-ffi/src/` 零命中） | 缺失 | beam-ffi 的导出面只有 `BeamClient` / `ClientConfig` / `DiscoveryListener` / `FrameSink` / `PeerInfo`（`crates/beam-ffi/src/lib.rs:58`），**不含任何 Opus 解码能力**。Android 侧实际是各自用 `MediaCodec` 自行解码，与文档建议的「复用 ffi 解码」不符。 | 未开始 |
| A-07 | 「边界:不做混音/回声消除」 | `crates/beam-audio/Cargo.toml:16-28`；`crates/beam-audio/src/` | 已实现 | crate 内无混音/AEC/回声处理代码；依赖仅 `beam-proto` / `bytes` / `thiserror` / `tracing`（+ optional `opus` / `windows`）。Android 侧 `android/core-audio/README.md:44` 亦明确「只解码与直通输出」。 | 未开始 |
| A-08 | 「错误码 `E7xxx`」 | `crates/beam-audio/src/error.rs:6-12,33-55` | 占位 | 四个错误码常量与变体齐备（7001 设备打开失败 / 7002 Opus 编解码失败 / 7003 音频输出失败 / 7004 后端未启用，`error.rs:6-12`、`33-55`），但**7001/7002/7003 无任何产生路径**：唯一会被抛出的是 `BackendDisabled`。 | 已完成 |
| A-09 | 「配置:`audio_mode=host+stream\|stream_only\|off`」 | `crates/beam-audio/src/audio.rs:27-34,44-53`；`crates/beam-config/src/model.rs:19-26,220-229` | 已实现 | 两侧都齐：crate 内 `AudioMode` 三值枚举（`audio.rs:27-34`，默认 `Off`，`audio.rs:36-40`）+ `AudioConfig.mode`（`audio.rs:45`）；全局配置模型 `AudioMode` 枚举（`model.rs:19-26`）、`AudioSettings.mode`（`model.rs:284`）、CLI `--audio`（`model.rs:151`）、环境变量 `BEAM_AUDIO`（`model.rs:187-189`）、字符串解析 `parse_audio_mode`（`model.rs:220-229`，三个取值 `host+stream`/`stream_only`/`off` 全部覆盖）。 | 未开始 |
| A-10 | 「配置:`bitrate=128k`」 | `crates/beam-audio/src/audio.rs:22-23,48`；`crates/beam-config/src/model.rs:286,331` | 已实现 | `DEFAULT_BITRATE_BPS = 128_000`（`audio.rs:23`）+ `AudioConfig.bitrate_bps`（`audio.rs:48`）；配置模型 `AudioSettings.bitrate_bps` 默认 `128_000`（`model.rs:286,331`）。 | 未开始 |
| A-11 | 「性能:采集→编码 ≤6 ms」 | `crates/beam-audio/src/audio.rs:201-202` | 缺失 | 只有一个常量 `CAPTURE_TO_ENCODE_BUDGET_MS = 6`（`audio.rs:202`），**没有任何计时/测量代码**；且采集与编码本身都未实现（见 A-01/A-02），该预算无从验证。 | 未开始 |
| A-12 | 「性能:端到端目标 ≤40 ms」 | `crates/beam-audio/src/audio.rs:198-199` | 缺失 | 只有常量 `END_TO_END_BUDGET_MS = 40`（`audio.rs:199`），无测量、无 E2E 打点。 | 未开始 |
| A-13 | 「测试:音画同步偏差 <30 ms」 | 未找到（已搜索：`sync`、`av_sync`、`30`、`drift`；`crates/beam-audio/src/` 仅 `audio.rs:151-166` 无关命中） | 缺失 | crate 内无同步偏差测量或测试；Android 侧也无音画同步统计。 | 未开始 |
| A-14 | 「里程碑:v1」（整节为 v1 能力） | `crates/beam-audio/Cargo.toml:12,14` | 占位 | §3.7 整节标注 v1，故不算「早做」，但当前实现形态是 `opus` feature 门控的占位（**默认关闭**）：`opus` 在 `Cargo.toml:28` 声明为 optional 且 `src/` 中从未被引用，开启后 `encode()`/`decode()` 仍返回 `E7004`（`audio.rs:170,192`）。见 A-01/A-02/A-03。 | 已完成 |

##### §3.8 beam-input

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| I-01 | 「输入事件模型(在 proto 定义)」 | `crates/beam-proto/src/input.rs:47-96` | 已实现 | `InputEvent` 六变体齐备（`MouseMove`/`MouseButton`/`Key`/`Scroll`/`GamepadAxis`/`GamepadButton`，`input.rs:49-96`），配套 `MouseButton`（`:16-26`）、`ButtonAction`（`:31-35`）、`KeyAction`（`:40-45`），并有 `is_gamepad()` 供按设备做权限判断（`:100-105`）。 | 未开始 |
| I-02 | 「主机侧 SendInput 注入键鼠」 | `crates/beam-input/src/sink.rs:116-133` | 占位 | `SendInputSink::apply()` 的 Windows 分支只有 `let _ = ev;` + 一行注释「真实实现：构造 INPUT 结构并调用 SendInput」（`sink.rs:123-125`）后 `Ok(())`。**没有任何 `SendInput` 调用**（已搜索 `SendInput`，仅命中类型名、注释与 `Cargo.toml` 的 windows feature 声明）。事件被静默丢弃且返回成功——这是六个 crate 里唯一会「假装成功」的路径。 | 已完成 |
| I-03 | 「vigem-client 虚拟 Xbox 手柄」 | `crates/beam-input/src/sink.rs:58-82` | 占位 | `vigem_client` 是**唯一被真正调用的原生依赖**（`sink.rs:71` 的 `vigem_client::Client::connect()`，失败映射为 `E8001`），但**只连总线、不创建 X360 虚拟手柄**——代码注释自己写明「X360 目标的创建与震动回传在 M3（v1）落地」（`sink.rs:68`）。`GamepadHub` 只在 `attached` 里记一个 `slot`（`sink.rs:74-75`），没有任何虚拟设备句柄。文档标注手柄属 v1（§3.8 第 145 行「里程碑:MVP(键鼠)/v1(手柄)」）。 | 未开始 |
| I-04 | 「震动回传」 | `crates/beam-input/src/sink.rs:34-38` | 占位 | 只有 `trait RumbleSink { fn rumble(&mut self, handle: GamepadHandle, strong: u16, weak: u16) -> Result<(), InputError>; }` 的接口声明，**没有任何实现**（已搜索 `impl RumbleSink`，`crates/` 全目录零命中）。 | 未开始 |
| I-05 | 「权限控制(按设备)」 | `crates/beam-input/src/sink.rs:99-113,116-120`；`crates/beam-pair/src/trust.rs:49` | 偏差 | 协议侧有按设备的权限位：`TrustedPeer.perms: PeerPerms`（`crates/beam-pair/src/trust.rs:49`），配置侧有 `InputSettings{gamepad, allow_keyboard, allow_mouse}`（`crates/beam-config/src/model.rs:271-278`）。但**注入路径完全不看这些**：`SendInputSink` 只有一个全局布尔 `allowed`（`sink.rs:101`），`apply()` 只判断 `if !self.allowed`（`sink.rs:118-120`），既不区分键/鼠/手柄，也不按来源设备查询 `perms`。单测 `sink_requires_permission`（`sink.rs:140-152`）也只验证了这个全局开关。 | 已完成 |
| I-06 | 「边界:不采集客户端输入」 | `crates/beam-input/src/`、`crates/beam-input/Cargo.toml:16-27` | 已实现 | crate 内无任何输入采集代码（无 `GetAsyncKeyState` / `Raw Input` / 事件钩子），只做注入与手柄管理。 | 未开始 |
| I-07 | 「边界:禁止依赖 capture/encode」 | `crates/beam-input/Cargo.toml:16-27` | 已实现 | 依赖仅 `beam-proto` / `thiserror` / `tracing`（+ optional `vigem-client`），无 `beam-capture`、无 `beam-encode`、无 `beam-session`、无 `beam-net`。 | 未开始 |
| I-08 | 「`trait InputSink { fn apply(&mut self, ev: InputEvent) -> Result<(), InputError>; }`」 | `crates/beam-input/src/sink.rs:12-15` | 已实现 | 逐字一致（`Send` 约束 + `apply` 签名）。 | 未开始 |
| I-09 | 「`GamepadHub::attach(slot) -> GamepadHandle`」 | `crates/beam-input/src/sink.rs:58-82` | 已实现 | 接口存在且行为合理。文档简写省略了 `Result`；代码为 `attach(&mut self, slot: u8) -> Result<GamepadHandle, InputError>`（`sink.rs:58`），这是 §3.8 强制要求 `E8001` 所必需的（ViGEmBus 未安装 / 槽位越界分别返回 `E8001`/`E8002`，`sink.rs:60,71,80`），判定为可接受的等价实现。`vigem` feature **默认开启**（`crates/beam-input/Cargo.toml:12,14`，与 §7「`vigem`(默认开)」一致）。 | 未开始 |
| I-10 | 「线程:输入 stream 接收任务 → 有界通道 → 注入线程(高优先级)」 | 未找到（已搜索：`thread::spawn`、`std::thread::`、`sync_channel`、`bounded`、`crossbeam`、`MMCSS`、`GameThread`、`priority`；`crates/beam-input/` 零命中） | 缺失 | **没有任何线程、没有任何通道、没有线程优先级设置**。输入事件只在调用方线程里同步处理。§6 要求的「抓屏/编码/解码/渲染/注入用专用 OS 线程并提升优先级(Windows MMCSS "Games")」在本 crate 完全未落地。 | 已完成 |
| I-11 | 「错误码 `E8xxx`(8001 ViGEmBus 未安装)」 | `crates/beam-input/src/sink.rs:69-73,80-81`；`crates/beam-input/src/error.rs:6,30-31` | 已实现 | **有真实产生路径**：`vigem_client::Client::connect()` 失败被 `map_err` 为 `InputError::ViGEmNotInstalled`（`sink.rs:71`）；feature 关闭时 `attach` 直接返回该错误（`sink.rs:80`）。文案映射齐备（`error.rs:16`）。这是六个 crate 中**唯一被真实触发过的错误码**。 | 未开始 |
| I-12 | 「8002 权限拒绝」 | `crates/beam-input/src/sink.rs:59-61,118-120`；`crates/beam-input/src/error.rs:8,34-35` | 已实现 | 有真实产生路径：槽位越界（`sink.rs:59-61`）与无权限注入（`sink.rs:118-120`）都返回 `PermissionDenied`。文案齐备（`error.rs:17`）。 | 未开始 |
| I-13 | 「性能:接收→注入 ≤1 ms」 | 未找到（已搜索：`bench`、`criterion`、`Instant`、`elapsed`；`crates/beam-input/` 零命中） | 缺失 | 无计时代码、无基准设施；且注入本身是空操作（I-02），该指标无从测量。 | 已完成 |
| I-14 | 「测试:回放脚本」 | 未找到（已搜索：`replay`、`script`、`fixture`、`json`；`crates/beam-input/src/` 零命中） | 缺失 | 无回放脚本、无录制/回放工具。 | 未开始 |
| I-15 | 「测试:手柄热插拔」 | `crates/beam-input/src/sink.rs:154-177`（`gamepad_attach_respects_slot_limit` / `gamepad_attach_and_detach`） | 缺失 | 现有单测只覆盖「槽位上限」与「attach/detach 幂等」，**不涉及热插拔**（无设备枚举、无插拔事件回调；`detach` 只是从 `Vec` 里移除，`sink.rs:85-89`）。且 `gamepad_attach_and_detach` 在本机没装 ViGEmBus 时会直接 `return` 跳过断言（`sink.rs:169-171`），覆盖度依赖环境。 | 未开始 |

##### §3.10 beam-decode / beam-render

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| V-01 | 「ffmpeg D3D11VA 硬解 HEVC/H.264」 | `crates/beam-decode/src/decoder.rs:76-89` | 占位 | `D3D11VaDecoder::decode()` 在 `ffmpeg` feature 开与关两个分支**都**返回 `Err(DecError::BackendDisabled)`（`decoder.rs:82,87`）。`ffmpeg-next` 在 `crates/beam-decode/Cargo.toml:25` 声明为 optional，`src/` 中从未被引用（已搜索 `ffmpeg_next`/`ffmpeg::`，`crates/` 全目录零命中）。`DecodeConfig.hardware = true`（`decoder.rs:21,29`）与 `low_latency = true`（`decoder.rs:22,30`）都只是标志位，无消费者。 | 已完成 |
| V-02 | 「(/AV1 v2)」 | `crates/beam-decode/src/decoder.rs:43-49`；`crates/beam-decode/Cargo.toml:16` | 占位 | `supports_codec()` 按 `av1` feature 门控 AV1（`decoder.rs:46`），与 §7「`av1`(v2)」一致，feature **默认关闭**（`Cargo.toml:12,16`）。但即便开启也无解码实现（V-01）。 | 已完成 |
| V-03 | 「解码纹理直接交给 D3D11 flip-model 交换链」 | `crates/beam-render/src/renderer.rs:80-95` | 占位 | `present()` 在 `d3d11` feature 下只做 `let _ = f;` + 注释「真实实现：D3D11 纹理 → flip-model 交换链 → Present」后 `self.presented += 1; Ok(())`（`renderer.rs:84-87`）。**不创建 D3D11 设备、不创建交换链、不做任何绘制**。`d3d11` 是**空 feature**（`crates/beam-render/Cargo.toml:14`：`d3d11 = []`，不引入任何依赖），因此连「开关是否真的接了原生库」都无法验证。 | 已完成 |
| V-04 | 「支持可变刷新率」（VRR） | `crates/beam-render/src/renderer.rs:16-29,36-38` | 占位 | 只有 `RenderConfig{refresh_rate: u32, vrr: bool}` 两个字段（`renderer.rs:24,26`，默认 `vrr = true`）与注释「目标刷新率（用于 VRR）」。**没有任何 DXGI 交换链创建、没有任何 `SetDisplayMode`/`SetFrameRate`/VRR 协商代码**（`Win32_Graphics_Dxgi` feature 虽在 `Cargo.toml:25-26` 启用但未被使用）。`vrr` 字段无任何消费者。 | 已完成 |
| V-05 | 「叠加层由 Tauri 窗口负责」 | `crates/beam-render/Cargo.toml:16-28`；`crates/beam-render/src/` | 已实现 | beam-render 内无任何 UI/叠加层代码，依赖仅 `beam-proto` / `thiserror` / `tracing` + windows 图形 API；职责划分与文档一致（`crates/beam-render/src/lib.rs:12-13` 亦明确「不做 UI 叠加层」）。 | 未开始 |
| V-06 | 「`trait VideoDecoder { fn decode(&mut self, f: AssembledFrame) -> Result<DecodedFrame, DecError>; }`」 | `crates/beam-decode/src/decoder.rs:36-41` | 已实现 | `decode` 签名逐字一致（`decoder.rs:38`）。`AssembledFrame` 定义在 L0 的 beam-proto（`crates/beam-proto/src/lib.rs:108-121`）以规避 §4 的 L1 横向依赖限制，有据。 | 已完成 |
| V-07 | 「`Renderer::present(&DecodedFrame)`」 | `crates/beam-render/src/renderer.rs:11-14` | 已实现 | `fn present(&mut self, f: &DecodedFrame) -> Result<(), RenderError>`（`renderer.rs:13`），签名吻合；`DecodedFrame` 同样来自 L0（`crates/beam-proto/src/lib.rs:145-157`）。 | 未开始 |
| V-08 | 「线程:解码线程 → 单槽 → 渲染线程(按 vblank)」 | 未找到（已搜索：`thread::spawn`、`std::thread::`、`sync_channel`、`单槽`、`WaitForSingleObject`、`Present`；`crates/beam-decode/`、`crates/beam-render/` 零命中） | 缺失 | **既无解码线程、无单槽交换、也无渲染线程**。唯一的「节拍」是 `FramePacer::should_present()`（`renderer.rs:113-124`），它只是比较两次时间戳差值是否小于 `interval_us`——这是**软件时间闸门，不是 vblank 对齐**，且其 `interval_us` 由调用方传入，代码中无任何地方调用它（`crates/beam-render/` 内除单测外无调用点）。 | 已完成 |
| V-09 | 「错误码 `E10xxx`」 | `crates/beam-decode/src/error.rs:6-14,32-52`；`crates/beam-render/src/error.rs:7-15,33-53` | 占位 | 解码侧 10001/10002/10003（`error.rs:8,10,12`）与渲染侧 10011/10012/10013（`error.rs:9,11,13`）的变体与文案齐备，但**没有任何产生路径**——默认路径只抛 `E10004`（`decoder.rs:87`）/`E10014`（`renderer.rs:92`）。 | 已完成 |
| V-10 | 「性能:解码 T1 ≤8、T2 ≤5、T3 ≤4 ms」 | 未找到（已搜索：`bench`、`criterion`、`p99`；全仓零命中） | 缺失 | 无基准设施、无计时点（与 E-23/E-25/C-18 同源）。 | 进行中 |
| V-11 | 「性能:上屏等待平均半帧」 | 未找到（已搜索：`latency`、`half_frame`、`vblank`、`present_time`；`crates/beam-render/src/` 零命中） | 缺失 | 无上屏延迟测量。 | 已阻塞 |
| V-12 | 「测试:各显卡解码基准」 | 未找到（已搜索：`bench`、`criterion`、`gpu`；全仓零命中） | 缺失 | 同 V-10。 | 已阻塞 |
| V-13 | 「测试:丢帧统计」 | `crates/beam-render/src/renderer.rs:74-77,127-129` | 占位 | 计数器确实存在且逻辑为真：`D3D11Renderer::dropped()`（`renderer.rs:74-77`）与 `FramePacer::dropped()`（`renderer.rs:127-129`），后者在 `should_present` 判定丢帧时自增（`renderer.rs:116`）。但**没有任何真实管线会调用它们**：默认 feature 下 `present()` 恒失败，`FramePacer` 无生产调用点，故统计恒为 0。 | 已完成 |

#### 文档未覆盖但代码里有的

以下为代码中存在、文档 §3.5/§3.6/§3.7/§3.8/§3.10 未要求的内容。多数是**为「无后端」兜底而设的扩展错误码**，本身无害，但会让 UI 把「功能未实现」显示成「后端未启用」，掩盖占位事实：

| # | 代码里有的 | 位置 | 是否造成偏差/风险 |
|---|---|---|---|
| X-01 | `E5004` 抓屏后端未启用 | `crates/beam-capture/src/error.rs:12,45-46` | 有风险：这是默认构建下 `start()` 的**唯一**返回（`source.rs:128`），语义上把「尚未实现」伪装成「没开 feature」 | 未开始 |
| X-02 | `E6004` 编码后端未启用 | `crates/beam-encode/src/error.rs:12,48-49` | 有风险：同上，且 `encoder.rs:159-160` 的注释还把空 probe 说成「对应 E6001」，与实际返回的 `E6004` 不符 | 未开始 |
| X-03 | `E7004` 音频后端未启用 | `crates/beam-audio/src/error.rs:12,54-55` | 有风险：同上 | 未开始 |
| X-04 | `E8003` 输入后端未启用 | `crates/beam-input/src/error.rs:10,38-39` | 有风险：`InputError::BackendDisabled` 从未被任何代码返回（`crates/beam-input/src/sink.rs` 全文无该变体构造），是死错误码 | 未开始 |
| X-05 | `E10004` 解码后端未启用 | `crates/beam-decode/src/error.rs:14,50-51` | 有风险：同上 | 未开始 |
| X-06 | `E10014` 渲染后端未启用 | `crates/beam-render/src/error.rs:15,51-52` | 有风险：同上 | 未开始 |
| X-07 | `VideoDecoder::reset()` 方法 | `crates/beam-decode/src/decoder.rs:40` | 无害：文档接口只有 `decode`，`reset` 是合理的设备丢失恢复补充（文档 §3.10 未禁止） | 未开始 |
| X-08 | `CaptureConfig.target_fps` | `crates/beam-capture/src/source.rs:52-53` | 无害：文档配置项只列了 `backend`/`display_id`/`min_update_interval`，`target_fps` 是节拍所需，§3.5 线程段也隐含需要 | 未开始 |
| X-09 | `EncodeConfig.idr_interval` | `crates/beam-encode/src/encoder.rs:121-122` | 无害：文档 §3.6 只提「强制 IDR」，未规定由请求触发还是按间隔触发 | 未开始 |
| X-10 | `RenderConfig.flip_model` 可关闭 | `crates/beam-render/src/renderer.rs:27-28` | 有风险：文档要求「解码纹理直接交给 D3D11 flip-model 交换链」是**固定**行为，做成可关闭的布尔开关反而可能关掉正确性前提 | 未开始 |
| X-11 | `#![forbid(unsafe_code)]` 六个 crate 全量启用 | `crates/beam-capture/src/lib.rs:48`、`crates/beam-encode/src/lib.rs:51`、`crates/beam-audio/src/lib.rs:38`、`crates/beam-input/src/lib.rs:44`、`crates/beam-decode/src/lib.rs:43`、`crates/beam-render/src/lib.rs:38` | **有风险，且是根因**：D3D11 纹理 / COM 指针 / 交换链的创建与生命周期管理必须用 `unsafe`，当前约束下 §3.5 与 §3.10 的 GPU 纹理链路在架构上无法实现 | 未开始 |
| X-12 | `D3D11Renderer.presented` / `FramePacer.dropped` 统计计数器 | `crates/beam-render/src/renderer.rs:70-77,127-129` | 无害：对应文档「丢帧统计」，但当前无管线驱动 | 未开始 |
| X-13 | Android `core-audio` 自行使用 `E70001/E70002/E70003/E70006` | `android/core-audio/src/main/java/dev/beam/core/audio/LowLatencyAudioPlayer.kt:85,94,108,146` | 有风险：与 Rust 侧 `E7001/E7002/E7003` 编号段重叠但语义映射不统一（Rust 侧 `E7002` = Opus 编解码失败，Kotlin 侧 `E70002` 同义但位数不同），跨端错误码不统一 | 未开始 |
| X-14 | `beam-audio` 额外定义 `AudioSource`/`OpusEncoder`/`OpusDecoder`/`AudioOutput` 四个 trait 族 | `crates/beam-audio/src/audio.rs:78-85,88-93,96-101,104-107` | 无害：文档 §3.7 只给了职责与边界、未给接口签名，文档化这些 trait 合理 | 未开始 |

#### 最该优先补的（最多 3 条）

1. **打通一条真实端到端的 GPU 帧链路（capture → encode → decode → render），并把 `#![forbid(unsafe_code)]` 改成局部 `deny` + 白名单。**
   当前六个平台 crate 的真实能力为零：所有原生依赖（`windows-capture` / `ffmpeg-next` / `opus`）虽然写进了 `Cargo.toml`，`src/` 里却一次都没有被引用；`beam-render` 的 `d3d11` 甚至是个不引入任何依赖的空 feature。在 `forbid(unsafe_code)` 下，D3D11 纹理与 COM 指针根本无法创建，因此这不是「少写了几个函数」，而是**架构上封死了 §3.5/§3.10 的核心需求**。应先解锁 unsafe，再让 `windows_capture` / `ffmpeg_next` 至少各有一条真实调用路径。

2. **把「后端未启用」和「功能未实现」区分开，别让占位以「E5xxx/E6xxx/E7xxx/E1xxxx 后端未启用」的形式冒充已完成。**
   现状是：六个 crate 的默认路径全部返回自定义的扩展码 `E5004`/`E6004`/`E7004`/`E10004`/`E10014`，UI 会显示「后端未启用」，而真实情况是**功能根本没写**。更糟的是 `crates/beam-encode/src/encoder.rs:159-160` 的注释直接断言空 probe「对应 §3.6 的 `E6001`」，与实际返回的 `E6004` 矛盾。建议：占位路径统一返回文档已定义的错误码（如 capture 用 `E5001`/`E5003`、encode 用 `E6001`），并在 crate 文档顶部用一张「已实现 / 占位」对照表自我披露，避免下游误判可用性。

3. **补上配置项的最后一公里，并把 `beam-config` 缺的三项补齐。**
   §3.5 的 `backend` / `display_id` / `min_update_interval` 与 §3.6 的 `slices` / `sfe` 在 `crates/beam-config/src/model.rs` 中**完全不存在**（该文件有 `codec`/`encoder`/`audio.mode`/`bitrate`，却独缺这五项），用户即使改了配置文件也不会有任何效果。同时这五个配置项即便接上，也需要有消费方（目前 `CaptureConfig`/`EncodeConfig` 的这些字段都没有读者）。建议在 beam-config 里补齐这五项，并让 `beam-host` 的真实实现读取它们。

---

##### 附：本次审计使用过的检索符号清单

`Dxgi` / `DXGI` / `dxgi` / `cursor` / `Cursor` / `光标` / `MinUpdateInterval` / `min_update_interval` / `VideoProcessor` / `NV12` / `nv12` / `compute` / `SendInput` / `send_input` / `Wasapi` / `WASAPI` / `loopback` / `MMCSS` / `GameThread` / `priority` / `windows_capture` / `ffmpeg_next` / `ffmpeg::` / `opus::` / `vigem_client::` / `thread::spawn` / `std::thread::` / `crossbeam` / `flume` / `channel::` / `sync_channel` / `bounded` / `criterion` / `[[bench]]` / `benches/` / `impl AudioOutput` / `impl RumbleSink` / `impl VideoEncoder for` / `impl VideoDecoder for` / `impl Renderer for` / `impl CaptureSource for` / `perms` / `allow_keyboard` / `allow_mouse` / `backend` / `display_id` / `slices` / `sfe` / `gamepad` / `opus` / `Opus` / `P1` / `ultra` / `CBR` / `cbr` / `lookahead` / `vbv` / `intra` / `b_frames` / `preset` / `ultralowlatency` / `vbr_latency` / `veryfast` / `amf` / `qsv` / `EnumDisplay` / `monitor` / `refresh` / `DeviceLost` / `recover` / `reset` / `recreate` / `win10` / `22H2` / `fps_limit` / `replay` / `script` / `fixture` / `json` / `latency` / `half_frame` / `vblank` / `present_time` / `gpu` / `p99` / `P99` / `bench` / `Instant` / `elapsed` / `sync` / `av_sync` / `drift`
### 7. session / host / client / ffi / gamestream（§3.9 / §4.1 / §3.12 uniffi / §2）

> 来自 `part-4.md`

#### 条目

##### §3.9 beam-session
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| S-01 | 职责:主/客会话状态机 | `crates/beam-session/src/state.rs:18-51`、`crates/beam-session/src/state.rs:55-97` | 已实现 | `SessionState` 9 态枚举 + `allowed_next()` 转移表 + `can_transition_to()` + `StateMachine::transition()`（`state.rs:136-145`）均为真实逻辑，非桩。 | 未开始 |
| S-02 | 串联 capture→encode→net | `crates/beam-session/src/host.rs:163-180`、`crates/beam-host/src/adapters.rs:50-83`、`crates/beam-host/src/adapters.rs:116-133` | 已实现 | `HostRunner::pump()` 真实调用 `pipeline.poll()` → `sender.send_frame()`；`CaptureEncodePipeline::poll` 真实调用 `capture.latest()` + `encoder.encode()`（`beam-host/src/adapters.rs:63-74`）。 | 未开始 |
| S-03 | 串联 net→decode→render | `crates/beam-session/src/client.rs:197-221`、`crates/beam-client/src/adapters.rs:105-107`、`crates/beam-client/src/adapters.rs:41-60` | 偏差 | 会话层结构在（`recv_frame()` → `consumer.consume()` → `decode()`+`present()`），但生产用的 `NetTransport::recv_frame` 恒返回 `None`（`beam-client/src/adapters.rs:105-107`），端到端收帧链路不可用；`send_control`/`send_input` 同样是空实现（`beam-client/src/adapters.rs:82-85`、`:96-99`）。 | 已完成 |
| S-04 | 能力协商 | 未找到（已搜索：`negotiate`、`beam_proto::negotiate`、`Hello`、`HelloAck` 于 `crates/beam-session/**`、`beam-host/**`、`beam-client/**`、`beam-ffi/**`） | 缺失 | session 只接受外部塞进来的 `SessionInput::Negotiated(StreamConfig)`（`crates/beam-session/src/event.rs:69`），从不调用 `beam_proto::negotiate`（`crates/beam-proto/src/caps.rs:214`），也不收发 `Hello`/`HelloAck`。 | 已完成 |
| S-05 | IDR 请求 | `crates/beam-session/src/event.rs:92`、`crates/beam-host/src/adapters.rs:76-78` | 占位 | `FramePipeline::request_idr()` 契约存在，host 适配器真实转发到 `encoder.request_idr()`；但 session 内无任何触发路径（`SessionInput` 无 IDR 变体，`ControlMsg::RequestIdr` 未被处理），全仓仅测试代码调用（`beam-host/src/adapters.rs:240`）。 | 已完成 |
| S-06 | 重连（里程碑:MVP(无重连)/v1） | `crates/beam-session/src/host.rs:136-145`、`crates/beam-session/src/client.rs:174-183` | 缺失 | 只有 `SessionInput::Stalled`/`Reconnected` 两个外部输入驱动的状态边；没有任何重新建连、定时器或 `Resume` 发送逻辑（已搜索：`Resume`、`reconnect`、`Instant`、`timer`、`timeout(` 于 `crates/beam-session/**`）。 | 已完成 |
| S-07 | 统计汇总 | `crates/beam-session/src/event.rs:30` | 占位 | `SessionEvent::Stats(StatsReport)` 变体存在，但全仓无任何 `SessionEvent::Stats` 的构造/发射点（已搜索：`SessionEvent::Stats`），host/client 也不做汇总。 | 已完成 |
| S-08 | 状态:`Idle → Discovering → Pairing → Connecting → Negotiating → Streaming ⇄ Reconnecting → Closed` | `crates/beam-session/src/state.rs:58-75` | 已实现 | 9 条状态边逐条落地：Idle→{Discovering,Pairing,Connecting,Closed}(:58-63)、Discovering→{Pairing,Connecting,Closed}(:64-68)、Pairing→{Connecting,Closed}(:69)、Connecting→{Negotiating,Closed}(:70)、Negotiating→{Streaming,Closed}(:71)、Streaming→{Reconnecting,Closed}(:72)、Reconnecting→{Streaming,Closed}(:73)、Closed→∅(:74)、Error→{Closed}(:75)。 | 未开始 |
| S-09 | `Error` 可从任意状态进入 | `crates/beam-session/src/state.rs:82-91` | 已实现 | `can_transition_to` 对 `next==Error` 特判，终态 `Closed` 除外一律允许；测试覆盖 `state.rs:242-254`。 | 未开始 |
| S-10 | `HostSession::start(cfg, deps) -> SessionHandle` | `crates/beam-session/src/host.rs:190-205` | 已实现 | 签名与文档逐字一致（`cfg: HostSessionConfig, deps: HostDeps`），返回真实 `SessionHandle` 并注入 runner。 | 未开始 |
| S-11 | `ClientSession::connect(peer, cfg) -> SessionHandle` | `crates/beam-session/src/client.rs:231-240` | 已实现 | 签名逐字一致（`peer: PeerAddr, cfg: ClientSessionConfig`）。 | 未开始 |
| S-12 | `ClientSession::connect(peer, cfg) -> SessionHandle`（可用性） | `crates/beam-session/src/client.rs:243-297` | 偏差 | 文档给出的唯一入口 `connect` 不接受 deps，无法注入传输 → 必然走 `MissingDependency`（`client.rs:286-295`，报 `E9002`）并停留在 `Idle`；真正可用的注入路径是文档未定义的 `connect_with`（`client.rs:243`）。 | 已完成 |
| S-13 | `SessionHandle::events()->Stream<SessionEvent>` | `crates/beam-session/src/handle.rs:149-162`、`crates/beam-session/src/handle.rs:223-229` | 已实现 | `SessionEventStream: Stream<Item=SessionEvent>` 真实实现 `poll_next`。风险：流只能取一次，第二次调用返回空流（`handle.rs:150-161`）。 | 未开始 |
| S-14 | `SessionHandle::update(StreamConfig)` | `crates/beam-session/src/handle.rs:165-176` | 已实现 | 真实写入 `SessionInner.config`；但该配置不向编码器/码率控制下发（无调用点，已搜索 `update(`），仅本地存取。 | 未开始 |
| S-15 | `SessionHandle::stop()` | `crates/beam-session/src/handle.rs:197-205` | 已实现 | 置 `stopped`、状态置 `Closed`、发 `SessionEvent::Closed`。 | 未开始 |
| S-16 | 边界:不直接调用平台 API,通过 trait(CaptureSource/VideoEncoder/VideoSender/…)注入 | `crates/beam-session/Cargo.toml:11-16`、`crates/beam-session/src/event.rs:86-135` | 已实现 | crate 依赖仅 `beam-proto/bytes/futures/thiserror/tracing`，零平台 crate；平台能力全部经 `FramePipeline`/`VideoSender`/`FrameConsumer`/`InputSink`/`Transport`/`TransportFactory` 注入，适配器在 `beam-host/src/adapters.rs:50-162`、`beam-client/src/adapters.rs:36-61`。 | 未开始 |
| S-17 | 边界:被 host/client/ffi 依赖 | `crates/beam-host/Cargo.toml:13`、`crates/beam-client/Cargo.toml:13`、`crates/beam-ffi/Cargo.toml:18` | 已实现 | 三者均依赖 `beam-session`，无反向依赖。 | 未开始 |
| S-18 | 错误码 `E9xxx` | `crates/beam-session/src/error.rs:4-15`、`crates/beam-session/src/error.rs:72-80` | 已实现 | 9001–9005 五码齐备，`is_session_code` 判段（`error.rs:102-104`）；`SessionEvent::Error{code,retryable}` 形态符合 §6（`event.rs:32-37`）。 | 未开始 |
| S-19 | 配置:`reconnect_timeout=30s`、`stall_threshold=3s` | `crates/beam-session/src/handle.rs:18-32`、`crates/beam-session/src/host.rs:22-33`、`crates/beam-session/src/client.rs:31-47` | 占位 | 三处常量与默认值齐备，但这两个值在 session 内**没有任何读取点**（全仓 `stall_threshold`/`reconnect_timeout` 命中均为定义与赋值处），即「只有常量，没有按阈值实现的判定」。 | 已完成 |
| S-20 | 测试:状态机表驱动测试 | `crates/beam-session/src/state.rs:153-233` | 已实现 | 9×9 全矩阵对照 `LEGAL` 表（`state.rs:153-215`）+ 终态/Error 专项（`:235-254`）。 | 未开始 |
| S-21 | 测试:假实现集成测试 | `crates/beam-session/src/host.rs:221-302`、`crates/beam-session/src/client.rs:311-428` | 偏差 | 假实现（`FakePipeline`/`FakeSender`/`FakeTransport`/`FakeConsumer`）确实存在并驱动主链路，但全部写在 `src/` 内 `#[cfg(test)]` 模块；`crates/beam-session/` 下无 `tests/` 目录，无独立集成测试目标，也没有 §8 所述「假 capture/encoder 本机回环跑完整会话」。 | 已完成 |

##### §4.1 时序与状态机
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| T-01 | 状态机:`Idle→Connecting` | `crates/beam-session/src/state.rs:62`、`crates/beam-session/src/client.rs:279` | 已实现 | Idle 的 `allowed_next` 含 `Connecting`；客户端连接成功后真实推进一步。 | 已完成 |
| T-02 | 状态机:`→(Pairing)→` | `crates/beam-session/src/state.rs:58-63`、`crates/beam-session/src/state.rs:69` | 已实现 | Idle→Pairing、Pairing→Connecting 均在表内。 | 已完成 |
| T-03 | 状态机:`Connecting→Negotiating` | `crates/beam-session/src/state.rs:70`、`crates/beam-session/src/client.rs:281` | 已实现 | 边存在且客户端主链路真实走到 `Negotiating`。 | 未开始 |
| T-04 | 状态机:`Negotiating→Streaming` | `crates/beam-session/src/state.rs:71`、`crates/beam-session/src/client.rs:284` | 已实现 | 边存在且被真实触发。 | 未开始 |
| T-05 | 状态机:`Streaming→Reconnecting` | `crates/beam-session/src/state.rs:72`、`crates/beam-session/src/host.rs:136-140` | 已实现 | 边存在，`SessionInput::Stalled` 可触发（需外部喂，判定触发条件见 T-09）。 | 未开始 |
| T-06 | 状态机:`Reconnecting→Streaming` | `crates/beam-session/src/state.rs:73`、`crates/beam-session/src/client.rs:179-183` | 已实现 | 边存在，`SessionInput::Reconnected` 可触发。 | 未开始 |
| T-07 | 状态机:`Reconnecting→Closed(超时)` | `crates/beam-session/src/state.rs:73` | 已实现 | 边在转移表内；「超时」这一条件本身无实现（见 T-08）。 | 未开始 |
| T-08 | `Streaming→Reconnecting(3 s 无数据)→Streaming(30 s 内恢复)\|Closed(超时)` 的 30 s 超时判定 | 未找到（已搜索：`reconnect_timeout`、`Instant`、`timer`、`timeout(`、`sleep` 于 `crates/beam-session/**`） | 缺失 | `reconnect_timeout` 只被存进 `SessionTimeouts`（`crates/beam-session/src/handle.rs:20`），没有任何计时/超时判定代码；`E9003` 会话超时错误（`crates/beam-session/src/error.rs:51-56`）也无产生点。 | 已完成 |
| T-09 | 3 s 无帧判定（`stall_threshold`） | 未找到（已搜索：`stall_threshold`、`Instant`、`last_frame`、`elapsed` 于 `crates/beam-session/**`） | 缺失 | `stall_threshold` 仅有常量（`crates/beam-session/src/handle.rs:22`），无「最后收帧时间」记录、无 3 s 判定；`Streaming→Reconnecting` 只能由外部手工喂 `SessionInput::Stalled`。 | 已完成 |
| T-10 | 状态机:任意→Error | `crates/beam-session/src/state.rs:86-89` | 已实现 | 非终态均可进入 `Error`，并发 `SessionEvent::Error`（`crates/beam-session/src/host.rs:96-108`）。 | 未开始 |
| T-11 | 状态机:Error→Closed | `crates/beam-session/src/state.rs:75` | 已实现 | `Error` 的后继只有 `Closed`。 | 未开始 |
| T-12 | 状态机:`Idle→Connecting→(Pairing)→Negotiating→Streaming`（§4.1 字面顺序） | `crates/beam-session/src/state.rs:69-70` | 偏差 | §4.1 与时序图把配对放在 QUIC 握手**之后**（文档第 229-232 行：握手→PairRequest→PairOk），而代码转移表要求 `Pairing→Connecting`，`Connecting→Pairing` 与 `Pairing→Negotiating` 均为非法转移。即代码采用 §3.9 的「先配对后连接」次序，与 §4.1 顺序相反；`crates/beam-session/src/state.rs:6-8` 声称已合并两处描述，但并未真正兼容 §4.1 的路径。 | 未开始 |
| T-13 | 时序:QUIC 握手(TLS1.3,首次未信任) | `crates/beam-client/src/adapters.rs:135-147`、`crates/beam-client/src/runtime.rs:124-133` | 已实现 | `NetTransportFactory::connect` 真实调用 `beam_net::NetEndpoint::connect`（`crates/beam-net/src/endpoint.rs:78-124`，含 rustls 校验器与 ALPN `beam/1`）。session 层通过 `TransportFactory` 间接使用。 | 未开始 |
| T-14 | 时序:`PairRequest(token 或 SPAKE2 msg)` | 未找到（已搜索：`PairRequest`、`PairChallenge`、`PairOk`、`PairFail` 于 `crates/beam-{session,host,client,ffi}/**`） | 缺失 | 5 个 crate 内零命中；类型只存在于 `crates/beam-proto/src/message.rs:79-82`，无发送/处理路径。 | 已完成 |
| T-15 | 时序:主机确认 / PIN 校验 | `crates/beam-ffi/src/client.rs:178-187` | 缺失 | session/host 侧无「用户确认」入口（无 `approve`/`reject` 类输入）；FFI 的 `pair_pin` 只做本地 PIN 长度与指纹格式校验（`beam-ffi/src/client.rs:179-185`），不发起 SPAKE2 交换（`beam_pair::PinPairing` 存在但未被调用，已搜索 `PinPairing` 于 `crates/beam-ffi/**`）。 | 已完成 |
| T-16 | 时序:`PairOk(双方固定指纹)` | 未找到（已搜索：`PairOk` 于 `crates/beam-{session,host,client,ffi}/**`） | 缺失 | 类型在 `crates/beam-proto/src/message.rs:89-92`；5 个 crate 内无处理、也不写信任库（`BeamClient::trust_store()` 返回只读引用，`crates/beam-ffi/src/client.rs:134-136`）。 | 已完成 |
| T-17 | 时序:`Hello{ver,caps}` | 未找到（已搜索：`Hello`、`HelloAck` 于 `crates/beam-{session,host,client,ffi}/**`） | 缺失 | 协议类型在 `crates/beam-proto/src/message.rs:72`；5 个 crate 内既不发送也不解析（`Transport::recv_control` 全仓无调用点）。 | 已完成 |
| T-18 | 时序:`HelloAck{agreed StreamConfig}` | 未找到（已搜索：`HelloAck` 于 `crates/beam-{session,host,client,ffi}/**`） | 缺失 | 类型在 `crates/beam-proto/src/message.rs:74-77`；无协商实现（见 S-04）。 | 已完成 |
| T-19 | 时序:视频 datagram 首帧 IDR | 未找到（已搜索：`request_idr`、`is_key`、`IDR` 于 `crates/beam-session/**`、`crates/beam-host/**`） | 缺失 | 主机侧推帧不保证首帧为 IDR，也不主动请求 IDR（`crates/beam-session/src/host.rs:174-179` 只做 `poll` + `send_frame`）；`request_idr` 无生产调用点。 | 已完成 |
| T-20 | 时序:输入 stream / Feedback | `crates/beam-client/src/adapters.rs:96-99` | 缺失 | `Transport::send_input` 契约在 `crates/beam-session/src/event.rs:124`，但 `NetTransport::send_input` 是空实现（`beam-client/src/adapters.rs:96-99`）；主机侧 `HostDeps.input` 声明后从未被调用（已搜索 `deps.input`、`\.input\.` 于 `crates/beam-session/src/**`），`Feedback` 无任何发送路径。 | 已完成 |
| T-21 | 时序:新连接/迁移 + `Resume{session_id}` | 未找到（已搜索：`Resume`、`session_id` 于 `crates/beam-{session,host,client,ffi}/**`） | 缺失 | 类型存在：`crates/beam-proto/src/message.rs:125-128`（`Resume{session_id:u64}`）；但 5 个 crate 内既不构造也不处理该消息，重连续传未实现。 | 已完成 |
| T-22 | 时序:重连后主机发 IDR | 未找到（已搜索：`request_idr`、`Reconnected` 于 `crates/beam-session/**`） | 缺失 | `SessionInput::Reconnected` 只把状态改回 `Streaming`（`crates/beam-session/src/client.rs:179-183`），不触发 IDR 请求。 | 已完成 |
| T-23 | 时序:`Bye` | 未找到（已搜索：`Bye` 于 `crates/beam-{session,host,client,ffi}/**`） | 缺失 | 协议类型在 `crates/beam-proto/src/message.rs:130`；5 个 crate 内无发送路径（`beam-net` 侧只有传输层 `Conn::close`，`crates/beam-net/src/endpoint.rs:149`，不是协议 `Bye`）。 | 已完成 |

##### §3.12 uniffi 导出
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| F-01 | `BeamClient::new(config)` | `crates/beam-ffi/src/client.rs:121-126` | 已实现 | 真实构造（持有 `ClientConfig` + `TrustStore`）。签名差异：返回 `Result<Self, FfiError>` 而非 `Self`。 | 未开始 |
| F-02 | `browse(listener)` | `crates/beam-ffi/src/client.rs:141-158` | 已实现 | 真实启动 `beam_discovery::Browser` 并在后台线程把 `DiscoveryEvent::Found/Lost` 回调给 listener。风险：线程无停止接口。 | 未开始 |
| F-03 | `pair_qr(payload)` | `crates/beam-ffi/src/client.rs:163-172` | 已实现 | 真实解析 `beam://pair?...` 载荷（`PairPayload::from_uri`）并返回 `PeerInfo`；不发起网络配对。 | 未开始 |
| F-04 | `pair_pin(peer,pin)` | `crates/beam-ffi/src/client.rs:178-187` | 偏差 | 仅本地校验 PIN 长度（`beam_pair::validate_pin`）与指纹十六进制格式，不做 SPAKE2 交换、不写信任库、不返回配对结果 → 「配对」未真正完成（`PinPairing` 未被使用，已搜索于 `crates/beam-ffi/**`）。 | 已完成 |
| F-05 | `connect(peer,cfg,sink: FrameSink)` | `crates/beam-ffi/src/client.rs:190-202` | 偏差 | `cfg` 与 `sink` 被直接丢弃（`client.rs:196`：`let _ = (cfg, sink);`），调用 `ClientSession::connect` 且不注入传输 → 会话停留 `Idle` 并报 `E9002`（`crates/beam-session/src/client.rs:286-295`）；`FrameSink` 永不收到任何帧/事件。 | 已完成 |
| F-06 | `callback interface FrameSink { on_frame(data: Vec<u8>/ByteBuffer, pts_us, capture_ts_us, key) ; on_event(SessionEvent) }` | `crates/beam-ffi/src/client.rs:68-77` | 已实现 | trait 形态与方法参数（`data, pts_us, capture_ts_us, key` / `event`）与文档一致；`data` 为 `&[u8]`（非 `Vec<u8>`/`ByteBuffer`），且该 trait 不是 uniffi callback interface（见 F-09）。 | 未开始 |
| F-07 | 热路径用 JNI 直接 ByteBuffer(DirectByteBuffer)避免 uniffi 拷贝 | `crates/beam-ffi/src/lib.rs:33-47` | 缺失 | 仅模块文档里的 4 步设计（Rust 侧缓冲池 → `NewDirectByteBuffer` 不复制 → Kotlin 归还 → 超时回收），**无任何实现代码**（已搜索：`DirectByteBuffer`、`NewDirectByteBuffer`、`jni`、`Pool` 于 `crates/beam-ffi/**`）；crate 还带 `#![forbid(unsafe_code)]`（`crates/beam-ffi/src/lib.rs:53`），当前无法直接做 JNI 零拷贝。文档第 46-47 行自述「本轮先落地接口契约与回调桥接，池在 M1 实现」。 | 暂不做 |
| F-08 | 控制面走 uniffi | 未找到（已搜索：`uniffi::export`、`uniffi::setup`、`uniffi::` 于 `crates/beam-ffi/**`） | 缺失 | 全仓无任何 uniffi 导出宏/UDL/`build.rs`（`crates/beam-ffi/` 下只有 `src/lib.rs`、`src/client.rs`、`src/error.rs`、`Cargo.toml`），控制面目前也不是 uniffi 类型。 | 已完成 |
| F-09 | uniffi 导出面（`beam-ffi` → Kotlin `core-net`） | `crates/beam-ffi/Cargo.toml:14`、`crates/beam-ffi/Cargo.toml:26` | 缺失 | 只有可选依赖 `uniffi` 与 `uniffi` feature；`BeamClient`/`FrameSink` 均为普通 Rust 类型，无导出注解，Kotlin 侧无法绑定（`android/core-net/README.md:51` 提到的 `beam_ffi.udl` 文件不存在）。 | 已完成 |
| F-10 | 边界:`beam-ffi` 只含 proto/net/pair/discovery，平台 crate 不得进入 ffi | `crates/beam-ffi/Cargo.toml:16-26` | 已实现 | 依赖恰为 `beam-proto/beam-session/beam-net/beam-pair/beam-discovery/bytes/futures/thiserror/tracing/uniffi`，无 capture/encode/input/decode/render。 | 未开始 |
| F-11 | 线程:帧回调在专用线程 → `queue()` | `crates/beam-ffi/src/client.rs:95-97` | 缺失 | `EventBridge::forward_frame` 在调用者线程同步直调 sink，无专用帧回调线程（`browse` 的后台线程只服务发现，`crates/beam-ffi/src/client.rs:144-155`）。 | 已完成 |

##### §2 / §3.11 相关
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| DX-01 | `beam-host` = 主机编排（第 51 行） | `crates/beam-host/src/runtime.rs:93-149` | 已实现 | `HostRuntime` 组装 capture/encode/net/input 并起会话（`runtime.rs:122-148`），mDNS 广播（`:103-114`）。风险：`start_session` 只有自测调用（全仓无生产调用方）。 | 未开始 |
| DX-02 | `host-->audio`（§4 第 216 行；§2 主机编排含 audio） | `crates/beam-host/Cargo.toml:16` | 缺失 | 声明了 `beam-audio` 依赖，但源码零引用（已搜索：`beam_audio`、`Audio` 于 `crates/beam-host/src/**`）→ 音频采集/编码未进主机编排。 | 已完成 |
| DX-03 | `beam-client` = Windows 客户端编排（第 52 行） | `crates/beam-client/src/runtime.rs:73-134` | 已实现 | `ClientRuntime` 提供浏览（`:85-95`）、建连（`:98-121`）、传输工厂（`:124-133`）；`DecodeRenderConsumer` 真实 decode+present（`crates/beam-client/src/adapters.rs:36-61`）。 | 未开始 |
| DX-04 | `beam-ffi` = uniffi 导出给 Android（第 53 行） | `crates/beam-ffi/src/lib.rs:1-59`、`crates/beam-ffi/Cargo.toml:16-26` | 偏差 | crate 位置与依赖集符合 §4（`ffi-->session/net/pair/discovery`），但「uniffi 导出」这一核心职责无实现（见 F-08、F-09）。 | 未开始 |
| DX-05 | `beam-gamestream` = GameStream 兼容层(v1 新增,仅按公开协议行为实现)（第 54 行） | `crates/beam-gamestream/src/capability.rs:1-114`、`crates/beam-gamestream/src/lib.rs:12-21` | 占位 | 全 crate 只有能力映射表与两个常量（`capability.rs:43-57`、`:91-113`）与 `gamestream` feature 占位（`Cargo.toml:11-14`）；无任何握手报文/协议行为实现，模块自述「最小骨架…不含任何真实握手报文」（`lib.rs:20-21`）。 | 未开始 |
| DX-06 | 兼容策略:不参考 GPL 代码（第 279 行 / §5 第 261 行） | `crates/beam-gamestream/src/lib.rs:5-10`、`crates/beam-gamestream/src/capability.rs:1-4` | 已实现 | 从代码本身看：crate 内只有能力结构体、映射函数与常量，无从 Sunshine/Moonlight 引入的协议常量表或报文模板；净室声明写在 `lib.rs:5-10`。 | 未开始 |
| DX-07 | §3.11 设置→运行参数 `HostConfig::from_resolved` | `crates/beam-host/src/runtime.rs:62-83` | 已实现 | 真实把 `ResolvedConfig` 的分辨率/帧率/码率/codec/音频模式映射到 `StreamConfig` + `CaptureConfig`。 | 未开始 |
| DX-08 | §3.11 设置→运行参数 `ClientConfig::from_resolved` | `crates/beam-client/src/runtime.rs:47-64` | 已实现 | 同上映射到 `StreamConfig`。 | 未开始 |
| DX-09 | 桌面端开播路径使用 host/client 编排（§3.11 `start_hosting`） | `apps/desktop/src-tauri/src/service_crates.rs:255`、`apps/desktop/src-tauri/src/service_crates.rs:319`、`apps/desktop/src-tauri/src/service_crates.rs:341` | 偏差 | 桌面端只用了 `HostConfig::from_resolved` 取参数，并**直接**调 `Advertiser::start`，未调用 `HostRuntime::start_session` / `ClientRuntime::connect`（已搜索：`start_session`、`HostRuntime`、`ClientRuntime`、`transport_factory` 于 `apps/desktop/**`）→ 编排层的会话装配能力当前没有生产调用方。 | 已完成 |
| DX-10 | §3.11 Events:`session://stats`(500 ms) | `crates/beam-session/src/event.rs:30` | 缺失 | `SessionEvent::Stats` 无产生点（见 S-07），host/client 不做 500 ms 统计节拍。 | 未开始 |

#### 文档未覆盖但代码里有的

1. **会话驱动的外部步进 API**：`SessionHandle::apply(SessionInput)` 与 `SessionHandle::pump()`（`crates/beam-session/src/handle.rs:115-128`、`:131-144`），以及 `SessionInput` 全部变体（`crates/beam-session/src/event.rs:54-80`）。文档只给了 `events/update/stop`，没有定义「谁在什么时机喂输入」；实际状态机推进完全依赖调用方，这使 §4.1 的时序无法由 session 自身保证。
2. **`ClientSession::connect_with`**（`crates/beam-session/src/client.rs:243-297`）：文档未列的第二个入口，且它是唯一能真正建立会话的路径（见 S-12）。
3. **`SessionHandle` 的一组只读访问器**：`config()`（`crates/beam-session/src/handle.rs:179-185`）、`state()`（`:188-194`）、`is_stopped()`（`:208-210`）、`timeouts()`（`:103-105`），文档未定义。
4. **FFI 配置字段 `auto_reconnect` / `enable_logging`**（`crates/beam-ffi/src/client.rs:20`、`:22`）：文档没有规定 `BeamClient` 配置字段，且这两个字段**声明后无任何读取点**（已搜索 `auto_reconnect|enable_logging`，仅命中定义处）→ 会被误当作已生效的开关。
5. **FFI 错误码段 `E14xxx`**（`crates/beam-ffi/src/error.rs:7-18`）与 **GameStream 错误码段 `E15xxx`**（`crates/beam-gamestream/src/lib.rs:36-41`）：文档未分配这两段编号段（文档只定义了 E1xxx–E10xxx、E11xxx、E12xxx），存在与后续新增 crate 撞号的风险。
6. **GameStream 协议常量 `GAMESTREAM_VERSION = "7.1.431.0"` 与 `_nvstream._tcp.local`**（`crates/beam-gamestream/src/capability.rs:9`、`:12`）：文档只说「按公开协议行为实现」，未给版本号/服务名；代码未标注这两个常量的公开出处，与「净室」声明之间缺少可核查的溯源注释。
7. **`beam-host` 声明但未使用的依赖** `beam-audio`、`beam-log`（`crates/beam-host/Cargo.toml:16`、`:21`）：在 `cargo check` 0 warning 的前提下仍能通过，说明工作区未开启未使用依赖告警；这类「声明即假装已接线」的做法会掩盖 X-02 这类缺失。
8. **`beam-client` 的 `beam-pair` 依赖实际只在测试中使用**（`crates/beam-client/Cargo.toml:18` vs 唯一使用点 `crates/beam-client/src/runtime.rs:174` 位于 `#[cfg(test)]` 模块）。

#### 最该优先补的（最多 3 条）

1. **把 §4.1 的时序落到 session 的控制面**：`Transport::recv_control` 全仓无调用点，`Hello/HelloAck/PairRequest/PairOk/RequestIdr/Feedback/Resume/Bye` 在 session/host/client/ffi 内零处理（见 T-14～T-23、S-04）。最小可行路径：在 `ClientRunner`/`HostRunner` 增加控制消息处理循环，用 `beam_proto::negotiate`（`crates/beam-proto/src/caps.rs:214`）完成能力协商，并在 `SessionInput::Negotiated` 之前真正跑一次协商。
2. **让 `stall_threshold=3s` / `reconnect_timeout=30s` 从常量变成判定**（S-19、T-08、T-09）：在 session 内记录「最后收帧时间」，超时进入 `Reconnecting` 并在 30 s 内尝试重建传输（配合 `Resume{session_id}`，T-21），否则 `Closed`。这是 §3.9「重连（v1）」与 §4.1 状态机的核心缺口，目前两个阈值完全没有消费方。
3. **打通 `beam-ffi` 的导出与热路径**（F-05、F-07、F-08、F-09）：`connect` 当前丢弃 `cfg`/`sink` 并必然失败；DirectByteBuffer 池只有设计文档。最低限度应让 `connect` 走 `ClientSession::connect_with`（注入传输 + 把帧转给 `FrameSink`），并为 uniffi 导出补 `#[uniffi::export]`/UDL 与 `build.rs`，同时解决 `#![forbid(unsafe_code)]`（`crates/beam-ffi/src/lib.rs:53`）与 JNI 零拷贝的冲突。
### 8. Android 12 模块（§3.12 / §7 Android / 产品方案 §7 §16）

> 来自 `part-5.md`

#### 条目

##### §3.12 各模块

文档原文表（`docs/beam-tech-design.md:166-179`）：

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

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| AN-01 | `:app` 职责·导航 | `android/app/src/main/java/dev/beam/app/BeamNavHost.kt:65` | 已实现 | 真实 `NavHost`（`startDestination = Routes.HOSTS`），路由表 `BeamNavHost.kt:34-49` 共 5 条（hosts/pairing/stream/settings/perf），各路由均装配真实 ViewModel 与 Screen。 | 未开始 |
| AN-02 | `:app` 职责·主题 | `android/app/src/main/java/dev/beam/app/theme/BeamTheme.kt:129` | 已实现 | `BeamTheme` 组装 Material3 `colorScheme`（`:25` 深色 / `:50` 浅色）+ `Typography`（`:78`），令牌取自 `core-data/.../ui/BeamTokens.kt`。主题模式由 `BeamRoot.kt:21-25` 按 `ThemeMode` 切换，非固定。 | 未开始 |
| AN-03 | `:app` 职责·权限 | `android/app/src/main/java/dev/beam/app/BeamPermissions.kt:23`；`android/app/src/main/AndroidManifest.xml:5` | 已实现 | manifest 声明 INTERNET / ACCESS_NETWORK_STATE / ACCESS_WIFI_STATE / CHANGE_WIFI_MULTICAST_STATE / NEARBY_WIFI_DEVICES / ACCESS_FINE_LOCATION(maxSdk 32) / CAMERA / POST_NOTIFICATIONS / FOREGROUND_SERVICE(_MEDIA_PLAYBACK)；运行时按需申请：相机 `BeamNavHost.kt:102`→`BeamPermissions.kt:50`，通知 `BeamRoot.kt:27`→`BeamPermissions.kt:69`。 | 未开始 |
| AN-04 | `:app` 职责·TV 入口 | `android/app/src/main/java/dev/beam/app/TvMainActivity.kt:15`；`android/app/src/main/AndroidManifest.xml:58` | 已实现 | **仅「文件存在」级别**：独立 Activity 已注册 `LEANBACK_LAUNCHER`（manifest `:65-68`）且 `android.software.leanback` 以 `required="false"` 声明（`:34-36`），同一 APK 支持手机+TV。**功能不完整**：`TvMainActivity.kt:17-24` 与 `MainActivity.kt:13-20` 完全同构（共用同一 `BeamRoot`），无 leanback 库、无 D-pad/焦点导航、无 TV 专版布局 —— 属 §3.12 里程碑里的 v1 范围（见 AU-18）。 | 未开始 |
| AN-05 | `:app` 对外接口：— | `android/app/README.md:31` | 已实现 | 文档标注「—」，即不要求对外 Kotlin API。`app` 模块未对外暴露任何业务类型，仅有壳层 Composable / DI 容器。 | 未开始 |
| AN-06 | `:app` 禁止：业务逻辑 | `android/app/src/main/java/dev/beam/app/AppContainer.kt:26` | 已实现 | `app` 主源集仅 8 个文件（BeamApplication/AppContainer/BeamRoot/BeamNavHost/BeamPermissions/MainActivity/TvMainActivity/theme/BeamTheme），均为装配与导航；状态与业务在 `feature-*/…ViewModel`、能力在 `core-*`。`BeamNavHost.kt:83` 的 `if (row.trusted) Routes.STREAM else Routes.PAIRING` 属导航策略而非业务规则。 | 未开始 |
| AN-07 | `:feature-discovery` 职责·主机列表 | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryScreen.kt:38` | 已实现 | `DiscoveryScreen` 真实渲染主机卡片列表（`DiscoveryScreen.kt:105-113` LazyColumn + `:118` HostCard）；数据来自 `DiscoveryViewModel.kt:72-93`（Rust mDNS 流 + NSD 兜底合并去重 `:108-122`）。 | 未开始 |
| AN-08 | `:feature-discovery` 职责·NSD 兜底 | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/HostScanner.kt:33` | 已实现 | `NsdHostScanner` 用系统 `NsdManager` 真实发现并解析 TXT（`HostScanner.kt:63` discoverServices、`:69-99` resolveService 读 `id/name/fp/platform`），服务类型 `_beam._udp.`（`:104`）；兜底与主路径并行合并于 `DiscoveryViewModel.kt:84-90`。 | 未开始 |
| AN-09 | `:feature-discovery` 对外接口：`DiscoveryViewModel` | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryViewModel.kt:55` | 已实现 | 类型名与文档完全一致；对外只暴露 `state: StateFlow<DiscoveryUiState>`（`:63`）与 `start/stop/refresh`（`:72/96/103`），另附手写 `factory`（`:153`）。 | 未开始 |
| AN-10 | `:feature-discovery` 禁止：直接调 JNI(经 core-net) | `android/feature-discovery/build.gradle.kts:40` | 已实现 | 全仓 `System.loadLibrary` 仅 1 处 `core-net/.../NativeBeamBridge.kt:439`，`Class.forName` 仅 1 处 `core-net/.../Reflect.kt:35`；discovery 只 `api(project(":core-net"))` 并只经 `BeamClient` 接口访问 Rust（`DiscoveryViewModel.kt:56`、`:78`）。禁止项落实。 | 未开始 |
| AN-11 | `:feature-pairing` 职责·CameraX+ML Kit 扫码 | `android/feature-pairing/build.gradle.kts:52` | 已实现 | 真实引入 `androidx.camera:camera-core/camera2/lifecycle/view`（`:52-55`）与 `com.google.mlkit:barcode-scanning`（`:56`）；使用点 `PairingScreen.kt:249` `BarcodeScanning.getClient()`、`:274-309` `ProcessCameraProvider` + `Preview` + `ImageAnalysis` + `scanner.process`，`:293` 只接受 `beam://pair` 前缀。 | 未开始 |
| AN-12 | `:feature-pairing` 职责·PIN 键盘 | `android/feature-pairing/src/main/java/dev/beam/feature/pairing/PairingScreen.kt:143` | 已实现 | `PinKeypad`（`:143`）+ `PinSlots`（`:175`）真实渲染 4–6 位输入格与 12 键键盘；VM 侧 `PairingViewModel.kt:126` onDigit / `:135` onBackspace / `:140` onSubmit，锁定与剩余次数按 `E3001/E3002` 处理（`:165-184`）。 | 未开始 |
| AN-13 | `:feature-pairing` 对外接口：`PairingViewModel` | `android/feature-pairing/src/main/java/dev/beam/feature/pairing/PairingViewModel.kt:78` | 已实现 | 类型名与文档一致；暴露 `state`（`:85`）+ `startFor/startScanning/onQrScanned/onDigit/onBackspace/onSubmit/reset`。 | 未开始 |
| AN-14 | `:feature-pairing` 禁止：— | — | 已实现 | 文档未列禁止项。核实无越界：不触碰 JNI、不含解码/渲染逻辑、协议模型复用 `:core-net` 的 `Peer`/`PairResult`。 | 未开始 |
| AN-15 | `:feature-stream` 职责·SurfaceView | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamScreen.kt:60` | 已实现 | 真实 `SurfaceView` + `SurfaceHolder.Callback`（`:61-78`），`surfaceCreated/surfaceChanged` 上送 Surface、`surfaceDestroyed` 通知停解码。 | 未开始 |
| AN-16 | `:feature-stream` 职责·悬浮工具栏 | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamScreen.kt:185` | 已实现 | `StreamToolbar`（`:185-203`）真实渲染展开/收起、统计浮层开关、断开按钮，用 `BeamTokens.OverlaySurface` + `OverlayOutline` 浮层底色。 | 未开始 |
| AN-17 | `:feature-stream` 职责·统计浮层 | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamScreen.kt:135` | 已实现 | `StatsOverlay`（`:135-177`）真实消费 `SessionStats`：端到端延迟、解码/显示耗时、分辨率、编码、丢包与 FEC 恢复率；数据由 `StreamViewModel.kt:216-218` 从 `SessionEvent.Stats` 写入。 | 未开始 |
| AN-18 | `:feature-stream` 对外接口：`StreamScreen` | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamScreen.kt:45` | 已实现 | 类型名与文档一致；纯展示 Composable（状态 + 回调），无状态内部逻辑。 | 未开始 |
| AN-19 | `:feature-stream` 禁止：解码细节 | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamViewModel.kt:11` | 偏差 | 文档禁止「解码细节」。但 `StreamViewModel.kt:11` 直接 `import dev.beam.core.decoder.MediaCodecVideoDecoder`，`:88` 默认工厂 `{ MediaCodecVideoDecoder() }`，`:245-254` 在 feature 层做 `VideoCodec → DecoderCodec` 映射并构造 `VideoFormat`（含 profile/level/fps）。`feature-stream/README.md:42` 自称「本模块**不**接触 `MediaCodec`」，与代码不符。解码器应由 `:core-decoder` 侧以工厂/`VideoFormat` 构造器下沉。 | 已完成 |
| AN-20 | `:feature-settings` 职责·设置页 | `android/feature-settings/src/main/java/dev/beam/feature/settings/SettingsScreen.kt:40` | 已实现 | 真实设置页：画质（分辨率 `:78`/帧率 `:82`/码率 `:88`）、编码（`:98`）、FEC（`:109`）、解码器偏好（`:119`）、统计浮层/音频/主题开关（`:139-152`）、恢复默认（`:156`）；写操作全落 `SettingsRepo`（`SettingsViewModel.kt:79-83`）。 | 未开始 |
| AN-21 | `:feature-settings` 对外接口：— | — | 已实现 | 文档未要求对外接口；模块仅被 `:app` 经 `BeamNavHost.kt:149-152` 使用。 | 未开始 |
| AN-22 | `:feature-settings` 禁止：— | — | 已实现 | 文档未列禁止项；核实无越界（无解码/网络/JNI）。 | 未开始 |
| AN-23 | `:feature-perf` 职责·性能面板 | `android/feature-perf/src/main/java/dev/beam/feature/perf/PerfPanel.kt:41` | 偏差 | 面板 Composable 真实存在（KPI 网格 `:81`、E2E 折线 `:127`、阶段表 `:174`、导出按钮 `:74-75`），但**没有任何数据生产者**：`PerfRecorder`（`feature-perf/.../PerfRecorder.kt:28`）全仓无实例化点，`PerfTickSource`（`core-net/.../Perf.kt:32`）无实现类（`PerfRecorder.kt:31` 默认 `emptyFlow()`），`Routes.PERF` 只有 `composable(Routes.PERF)`（`BeamNavHost.kt:169`）而无任何 `navigate(Routes.PERF)` 入口。面板恒为空态（`PerfPanel.kt:182-187`）。 | 已完成 |
| AN-24 | `:feature-perf` 对外接口：`PerfPanel` | `android/feature-perf/src/main/java/dev/beam/feature/perf/PerfPanel.kt:41` | 已实现 | 类型名与文档一致：`fun PerfPanel(state: PerfUiState, onExport: (PerfExportFormat) -> Unit, modifier: Modifier)`。 | 未开始 |
| AN-25 | `:feature-perf` 禁止：被 release 依赖 | `android/app/build.gradle.kts:86` | 偏差 | **依赖图层面合规**：全仓仅 `debugImplementation(project(":feature-perf"))` 一处引用（`:86`），release 变体用同名空实现 `android/app/src/release/java/dev/beam/app/perf/PerfEntry.kt:16-33`（`available=false`、`Panel` 空、`export` 返回 ""）。**但 perf 类型仍进 release**：`PerfExportFormat`/`PerfUiState`/`PerfSnapshotStore` 位于 `:core-data` 的 **main** 源集（`core-data/src/main/java/dev/beam/core/data/perf/PerfModels.kt:10/31/54`），`PerfTick`/`PerfTickSource` 位于 `:core-net` main 源集（`core-net/.../Perf.kt:13/32`）。§7:276 要求的 release APK 无 perf 类检查用 `grep -i "feature-perf\|perfstats"`（`.github/workflows/ci.yml:103`），匹配不到 `dev/beam/core/data/perf/PerfSnapshotStore.class`，门禁实际失效。 | 未开始 |
| AN-26 | `:core-net` 职责·uniffi 绑定 `BeamClient` | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:52` | 已实现 | Kotlin 侧适配层是真实实现：`NativeBeamBridge : BeamClient`（`:52`），按候选包名反射查找生成类（`:397`）、反射调用 `browse/pairQr/pairPin/connect`（`:62/76/83/106`）。**注**：产物侧的缺失另见 AU-01。 | 未开始 |
| AN-27 | `:core-net` 职责·回调转 Flow | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:481` | 已实现 | `SessionBus`（`:481-492`，`MutableSharedFlow` + `BufferOverflow.DROP_OLDEST`）；`NativeSession.events`（`:506`）、`stats`（`:508` filterIsInstance<Stats>）、`audioPackets`（`:512`）由 bus 派生；Rust 回调入口 `newFrameSinkProxy`（`:136`）的 `onEvent` 分支 `bus.events.tryEmit`（`:148`）。 | 未开始 |
| AN-28 | `:core-net` 对外接口：`interface BeamClient { suspend fun connect(peer: Peer, cfg: StreamCfg): Session }` | `android/core-net/src/main/java/dev/beam/core/net/BeamClient.kt:20` | 偏差 | 接口存在（`:20`）且 `Peer`/`StreamCfg`/`Session` 类型名一致，但**签名不符**：文档要求二参 `connect(peer, cfg): Session`，代码只有两个**都带 sink** 的重载 —— `:49 suspend fun connect(peer: Peer, cfg: StreamCfg, sink: FrameSink): Session` 与 `:56 suspend fun connect(peer: Peer, cfg: StreamCfg, sinkFactory: (StreamCfg) -> FrameSink): Session`。不存在二参版本（`NativeBeamBridge.kt:92` 的 `connect(peer, cfg) { sink }` 是调用工厂重载，非二参重载）。 | 已完成 |
| AN-29 | `:core-net` 禁止：— | — | 已实现 | 文档未列禁止项。核实无越界：`core-net/build.gradle.kts:32-41` 只依赖 coroutines + kotlin-reflect，不依赖 `:core-decoder`/`:core-renderer`，符合「本模块不含解码/渲染」。 | 未开始 |
| AN-30 | `:core-decoder` 职责·MediaCodec 异步模式 | `android/core-decoder/src/main/java/dev/beam/core/decoder/MediaCodecVideoDecoder.kt:88` | 已实现 | 真实异步模式：`HandlerThread("beam-decoder")`（`:83`）+ `setCallback`（`:88`），`onInputBufferAvailable`（`:89`）拉取输入缓冲、`onOutputBufferAvailable`（`:96`）释放输出；非轮询。 | 未开始 |
| AN-31 | `:core-decoder` 职责·`KEY_LOW_LATENCY` | `android/core-decoder/src/main/java/dev/beam/core/decoder/MediaCodecVideoDecoder.kt:66` | 已实现 | `setInteger(MediaFormat.KEY_LOW_LATENCY, 1)`（`:66`），并在 configure 之后 `setParameters(DeviceDecoders.lowLatencyParams())`（`:76`，参数包见 `DeviceDecoders.kt:110-112`），覆盖部分厂商只认 setParameters 的情况。 | 未开始 |
| AN-32 | `:core-decoder` 职责·厂商 low_latency 解码器 | `android/core-decoder/src/main/java/dev/beam/core/decoder/DecoderSelector.kt:45` | 已实现 | 白名单 `LOW_LATENCY_VENDORS`（`:45-54`：qti/qcom/mtk/exynos/hisilicon/hisi/amlogic/rk/marlin）+ 选型优先级「硬解+低延迟+白名单 → 硬解 → 软解」（`:64-87`）+ 厂商 token 精确段匹配（`:96-101`）；设备侧枚举 `DeviceDecoders.kt:27`、低延迟能力反射探测 `feature.low-latency`（`:86-91`）；实际使用 `MediaCodecVideoDecoder.kt:181-189`。 | 未开始 |
| AN-33 | `:core-decoder` 职责·output fence | `android/core-decoder/src/main/java/dev/beam/core/decoder/MediaCodecVideoDecoder.kt:96` | 已实现 | 通过 `codec.releaseOutputBuffer(index, true)`（`:98`）直出 Surface，由框架内部完成 fence 同步（公开 SDK 未暴露 MediaCodec 输出 fence API）。代码注释 `:97` 明示该口径，未见独立 fence 等待/超时逻辑 —— 属实现路径选择而非缺失。 | 未开始 |
| AN-34 | `:core-decoder` 对外接口：`interface VideoDecoder { fun start(surface: Surface, fmt: VideoFormat); fun queue(frame: ByteBuffer, ptsUs: Long, key: Boolean); fun stop() }` | `android/core-decoder/src/main/java/dev/beam/core/decoder/VideoDecoder.kt:29` | 已实现 | 逐字一致：`start(surface, fmt)`（`:37`）、`queue(frame, ptsUs, key)`（`:46`）、`stop()`（`:49`）。 | 未开始 |
| AN-35 | `:core-decoder` 禁止：网络 | `android/core-decoder/build.gradle.kts:33` | 已实现 | 模块依赖仅 coroutines（`:35-36`），**不依赖** `:core-net`（`:34` 注释明示该禁止项）；源码内无任何网络 import，输入只接受调用方交来的 `ByteBuffer`（`VideoDecoder.kt:42`）。 | 未开始 |
| AN-36 | `:core-renderer` 职责·Surface 管理 | `android/core-renderer/src/main/java/dev/beam/core/renderer/SurfaceFrameRate.kt:51` | 偏差 | 模块内 Surface 相关只有一个 `fun interface SurfaceSource`（`:51-55`），且**全仓无实现方、无使用方**（仅定义处一处命中）；真正的 Surface 生命周期（SurfaceHolder.Callback）在 `:feature-stream`（`StreamScreen.kt:61-78`）。即「Surface 管理」这一职责未落在 `:core-renderer`，与文档分层不符。 | 未开始 |
| AN-37 | `:core-renderer` 职责·`Surface.setFrameRate` | `android/core-renderer/src/main/java/dev/beam/core/renderer/SurfaceFrameRate.kt:23` | 已实现 | `apply(surface, fps)`（`:23-31`）真实调用 `surface.setFrameRate(fps.toFloat(), compatibility)`（`:30`），API 30 以下空操作（`:29`）；另有 `applyFixedSource`（`:39-42`）。调用点 `feature-stream/.../StreamViewModel.kt:161`。 | 未开始 |
| AN-38 | `:core-renderer` 职责·帧节奏 | `android/core-renderer/src/main/java/dev/beam/core/renderer/FramePacer.kt:77` | 已实现 | `decide(ptsUs, arriveUs)` 真实实现 Present/Drop/Skip/Wait 四态（`:77-96`），按到达时刻定节拍、pts 只做顺序校验（`:38-44` 注释）；被 `feature-stream/.../StreamViewModel.kt:122` 创建、`:268` 消费。 | 未开始 |
| AN-39 | `:core-renderer` 对外接口：`FramePacer` | `android/core-renderer/src/main/java/dev/beam/core/renderer/FramePacer.kt:54` | 已实现 | 类型名与文档一致；`PacerDecision` 密封类型同文件 `:8`。 | 未开始 |
| AN-40 | `:core-renderer` 禁止：— | — | 已实现 | 文档未列禁止项；核实不依赖 `:core-net`/`:core-decoder`（`core-renderer/build.gradle.kts:33-39`）。 | 未开始 |
| AN-41 | `:core-input` 职责·触控/手柄/遥控映射为 InputEvent | `android/core-input/src/main/java/dev/beam/core/input/InputMapper.kt:45` | 已实现 | 映射真实实现：触控归一化+死区（`:61-84`）、键/遥控 D-pad（`:87`、`:91`）、鼠标（`:95/99/103`）、手柄按键/摇杆含死区不下发（`:107`、`:115-119`）；`MotionActions.kt:21-24` 提供 `MotionEvent`→`TouchAction` 映射。映射规则本身达标（**采集与回传**见 PP-03）。 | 未开始 |
| AN-42 | `:core-input` 对外接口：`InputMapper` | `android/core-input/src/main/java/dev/beam/core/input/InputMapper.kt:45` | 已实现 | 类型名与文档一致；`InputMapperConfig`（`:25`）、`DpadDirection`（`:13`）配套。 | 未开始 |
| AN-43 | `:core-input` 禁止：— | — | 已实现 | 文档未列禁止项；核实只产出事件对象、不碰网络，复用 `:core-net` 的 `InputEvent`（`core-input/build.gradle.kts:35` api）。 | 未开始 |
| AN-44 | `:core-audio` 职责·Opus 解码 | `android/core-audio/src/main/java/dev/beam/core/audio/LowLatencyAudioPlayer.kt:136` | 已实现 | 真实 MediaCodec Opus 解码：`createDecoderByType("audio/opus")` + `KEY_PCM_ENCODING=PCM_16BIT`（`:136-148`），`drainDecoded`（`:150-178`）取 PCM 写入输出。 | 未开始 |
| AN-45 | `:core-audio` 职责·AAudio | `android/core-audio/src/main/java/dev/beam/core/audio/LowLatencyAudioPlayer.kt:66` | 偏差 | **不使用 AAudio**：改用 `AudioTrack`（import `:5`，`AudioTrack.Builder` `:66-83`，`PERFORMANCE_MODE_LOW_LATENCY` `:82`，`USAGE_GAME`+`CONTENT_TYPE_MOVIE` `:69-70`，`MODE_STREAM` `:81`）。理由记录在 `core-audio/README.md:22-38`（公开 SDK 无 AAudio Java 绑定）与源码注释 `LowLatencyAudioPlayer.kt:18-33`，并指出补 NDK/JNI 薄封装的后续路径。延迟量级近似但并非文档指定的 AAudio 实现 —— 属明示偏差而非静默替换。 | 未开始 |
| AN-46 | `:core-audio` 对外接口：`AudioPlayer` | `android/core-audio/src/main/java/dev/beam/core/audio/AudioPlayer.kt:30` | 已实现 | 类型名与文档一致：`start(format)`（`:33`）、`enqueue(packet, ptsUs)`（`:41`）、`stop()`（`:44`）、`release()`（`:47`）。 | 未开始 |
| AN-47 | `:core-audio` 禁止：— | — | 已实现 | 文档未列禁止项；核实不做混音/回声消除、不做网络（`core-audio/README.md:44-45`），只解码与直通输出。 | 未开始 |
| AN-48 | `:core-data` 职责·DataStore 设置 | `android/core-data/src/main/java/dev/beam/core/data/SettingsRepo.kt:37` | 已实现 | 基于 Jetpack DataStore Preferences：读流 `settings`（`:42-46`，IOException 回退默认不崩）、写 `update`（`:49-63`）、`reset`（`:66`）；容器在 `app/.../AppContainer.kt:30-35`。 | 未开始 |
| AN-49 | `:core-data` 职责·信任主机 | `android/core-data/src/main/java/dev/beam/core/data/HostRepo.kt:23` | 已实现 | 真实信任库：按指纹去重 `upsert`（`:35-41`）、`remove`（`:44-51`）、`find`（`:54`）、`isTrusted`（`:58`）、`clear`（`:61`）；编码用 `PeerCodec`（`core-data/.../PeerCodec.kt:38`）。 | 未开始 |
| AN-50 | `:core-data` 对外接口：`SettingsRepo`、`HostRepo` | `android/core-data/src/main/java/dev/beam/core/data/SettingsRepo.kt:37`；`.../HostRepo.kt:23` | 已实现 | 两个类型名与文档完全一致，均为 public class。 | 未开始 |
| AN-51 | `:core-data` 禁止：— | — | 已实现 | 文档未列禁止项；核实不触碰 JNI。**附带观察**（属 §3.14 范围，本次不单列）：§3.14 要求「Android 用 DataStore + Keystore」，但全仓 `KeyStore`/`AndroidKeyStore`/`Cipher` 零命中，指纹以明文存于 DataStore（`PeerCodec.kt:38-49`），私钥未由 Keystore 保护。 | 未开始 |
| AN-52 | `:app` 职责·权限（串流前台服务落地） | `android/app/src/main/AndroidManifest.xml:24` | 缺失 | manifest 声明了 `FOREGROUND_SERVICE` / `FOREGROUND_SERVICE_MEDIA_PLAYBACK`（`:24-25`），`BeamPermissions.kt:32-35` 也定义了二者并注明「随串流前台服务使用」，但 `android/app/src/main/` 下**没有任何 Service 实现**（该源集仅 8 个 kt 文件，无 Service 类）。权限声明无对应实现。 | 已完成 |

##### §3.12 uniffi 对接与线程/性能

文档原文（`docs/beam-tech-design.md:180-181`）：

> - uniffi 导出(beam-ffi):`BeamClient::new(config)`、`browse(listener)`、`pair_qr(payload)`、`pair_pin(peer,pin)`、`connect(peer,cfg,sink: FrameSink)`;`callback interface FrameSink { on_frame(data: Vec<u8>/ByteBuffer, pts_us, capture_ts_us, key) ; on_event(SessionEvent) }`。热路径用 JNI 直接 ByteBuffer(DirectByteBuffer)避免 uniffi 拷贝;控制面走 uniffi。
> - 线程:Rust tokio 运行时在 so 内;帧回调在专用线程 → `queue()`;UI 在 Main。性能:解码 T1 ≤8、T2 ≤5 ms(T3 Android 不作为验收目标)。里程碑:MVP(列表/配对/解码)/v1(TV、手柄、音频)。

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| AU-01 | uniffi 导出(beam-ffi) 的产物落地（Kotlin 绑定 + `libbeam_ffi.so`） | `crates/beam-ffi/src/lib.rs:53` | 缺失 | **Rust 侧根本没有 uniffi 导出面**，因此生成物永远无法产出：`crates/beam-ffi/src/lib.rs:53-59` 只有 `pub mod client/error` 与 re-export，无 `uniffi::setup_scaffolding!`、无 `#[uniffi::export]`/`derive(Uniffi)`（在 `crates/beam-ffi` 内搜 `uniffi::`/`setup_scaffolding`/`derive(...Uniffi)` 零命中）；`crates/beam-ffi/Cargo.toml` 声明了 `uniffi` 可选依赖与 `uniffi` feature（`:11-14`、`:26`，默认关闭）但无任何代码引用，且**无 `crate-type = ["cdylib"]`**（全文件仅 29 行）；仓库内**无任何 `.udl` 文件**；Kotlin 生成目录 `android/core-net/src/main/java/dev/beam/core/net/uniffi/` 不存在；`android/core-net/src/main/jniLibs/arm64-v8a/README.md:1-7` 与 `x86_64/README.md:1-7` 只有占位说明，无 `.so`。→ 串流链路当前不可用。 | 已完成 |
| AU-02 | `NativeBeamBridge` 用反射调用生成类 | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:52` | 已实现 | 真实反射适配：`Reflect.findClass` 依次试候选包名 `dev.beam.core.net.uniffi` / `uniffi.beam_ffi` / `dev.beam.ffi`（`:364-377`）、`findMethod` 按名+参数个数（`:328-333`）、`Reflect.invoke` 包装异常（`core-net/.../Reflect.kt:54-60`）。 | 未开始 |
| AU-03 | `System.loadLibrary("beam_ffi")` | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:439` | 已实现 | `const val LIBRARY_NAME = "beam_ffi"`（`:362`）、`System.loadLibrary(LIBRARY_NAME)`（`:439`），`UnsatisfiedLinkError` 被转成带修复指引的 `BeamNativeUnavailableException`（`:440-447`）。 | 未开始 |
| AU-04 | `Class.forName` | `android/core-net/src/main/java/dev/beam/core/net/Reflect.kt:35` | 已实现 | `Class.forName(name)` 逐候选尝试，`ClassNotFoundException`/`LinkageError` 均降级为「不可用」（`:33-42`）。 | 未开始 |
| AU-05 | 回调用 `java.lang.reflect.Proxy` | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:340` | 已实现 | `Proxy.newProxyInstance(iface.classLoader, arrayOf(iface))`（`:340`）真实创建动态代理；用于 `FrameSink`（`:136`）与 `DiscoveryListener`（`:123`），含 `toString/hashCode/equals` 默认返回（`:347-353`）。 | 未开始 |
| AU-06 | record 用 `kotlin-reflect` 按参数名装配 | `android/core-net/src/main/java/dev/beam/core/net/Reflect.kt:133` | 已实现 | `buildRecord` 读 `primaryConstructor` 并按 `param.name` 取值后 `ctor.callBy(args)`（`:133-148`），避免与 Rust 字段顺序耦合；枚举按名解析（`:151-157`）；依赖 `core-net/build.gradle.kts:38` `implementation(libs.kotlin.reflect)`。 | 未开始 |
| AU-07 | DirectByteBuffer 池化 | `android/core-net/src/main/java/dev/beam/core/net/buffer/ByteBufferPool.kt:18` | 已实现 | 真实池：`rent` 命中复用或 `ByteBuffer.allocateDirect`（`:35-51`，默认 1 MiB `:86`）、`release` 回池（`:61-68`，上限 4 MiB `:89`）、`BufferLease.close` 归还（`:103-117`）、线程安全（`synchronized(idle)`）。 | 未开始 |
| AU-08 | `deliverFrame` 接管所有权或从 `ByteBufferPool` 租直接内存 | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:171` | 已实现 | 两条路径都真实存在：Rust 传 `ByteBuffer` → 直接接管（`:181-184`）；Rust 传 `ByteArray` → `pool.rentLease` 租直接内存并拷贝（`:185-191`）；其余类型直接丢弃（`:192`）。 | 未开始 |
| AU-09 | 消费方实现 `PooledFrameSink` 则转移所有权，否则回调返回即归还 | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:195` | 已实现 | `if (sink is PooledFrameSink)` → `sink.onPooledFrame(PooledFrame(...))` 转移所有权（`:195-197`，接口见 `core-net/.../PooledFrameSink.kt:13-20`）；否则 `try { sink.onFrame(...) } finally { lease.close() }`（`:199-204`），与 §6「回调返回后归还」一致。 | 未开始 |
| AU-10 | 控制面 Rust 回调 → `SessionBus.events` → `session://stats`（500 ms） | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt:148` | 偏差 | 回调→Flow 链路完整（`onEvent` → `bus.events.tryEmit` `:148`；`SessionBus.events` `:482-486`；`stats` 派生 `:508-510`）。但**500 ms 节拍无人产生**：Kotlin 侧无任何定时器/节拍器（`android/` 内搜 `500`/`delay(500)`/`ticker` 零命中），Rust 侧 `crates/beam-session/src/event.rs:30` 只定义了 `Stats(StatsReport)` 变体，`crates/beam-session/src/` 内搜 `from_millis`/`interval`/`ticker` 零命中。节拍因此从未产生，`stats` 流实际为空。 | 已完成 |
| AU-11 | 生成物缺席时抛 `BeamNativeUnavailableException` 并在消息中给出修复命令 | `android/core-net/src/main/java/dev/beam/core/net/Reflect.kt:15` | 已实现 | 异常类定义 `Reflect.kt:15-16`；三处带修复指引的消息：`NativeBeamBridge.kt:398-401`（「请执行 core-net/README.md 的 uniffi-bindgen 生成步骤」）、`:425-427`（构造器形状不符）、`:441-446`（「请用 cargo-ndk 构建并把产物复制到 core-net/src/main/jniLibs/arm64-v8a/ 与 jniLibs/x86_64/」）、`:323-326`（record 类缺失）。`core-net/README.md:98-109` 有对应表格。 | 未开始 |
| AU-12 | App 降级为 `UnavailableBeamClient` 后仍可启动 | `android/app/src/main/java/dev/beam/app/AppContainer.kt:48` | 已实现 | `nativeReady = NativeBeamBridge.isAvailable()`（`:41`）→ 未就绪则 `UnavailableBeamClient()`（`:52`），创建异常也回落（`:49-50`）；`UnavailableBeamClient.kt:12-35` 为完整降级实现，`:33-34` 默认文案含「请按 core-net/README.md 生成 uniffi 绑定并用 cargo-ndk 构建 .so」。UI 侧 `DiscoveryScreen.kt:69-75` 显示「原生库尚未构建」告警而不崩溃。 | 未开始 |
| AU-13 | 线程：Rust tokio 运行时在 so 内 | `crates/beam-ffi/Cargo.toml:16` | 缺失 | `crates/beam-ffi` 与 `crates/beam-session` 内搜 `tokio` **零命中**，beam-ffi 也未启动任何 runtime（tokio 仅作为 `beam-net` 的依赖存在：`crates/beam-net/Cargo.toml:20`、`:24`）。即便补上 uniffi 导出，so 内也没有 §3.12 要求的 tokio 运行时；且 so 本身尚未产出（AU-01）。 | 未开始 |
| AU-14 | 线程：帧回调在专用线程 → `queue()` | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamViewModel.kt:266` | 已实现 | `StreamSink.onFrame` 在 Rust 派发的回调线程内按 `FramePacer` 决策后调用 `decoder.queue(buffer, ptsUs, key)`（`:269`）；非 Present 分支丢弃（`:270-277`），Wait 分支先拷贝再延迟提交（`:278-287`），符合「缓冲回调后失效」契约。`core-net/.../FrameSink.kt:15` 注明回调线程约束。 | 未开始 |
| AU-15 | 线程：UI 在 Main | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamViewModel.kt:118` | 已实现 | 状态更新走 `viewModelScope`（Main 调度器，`:118`、`:213`、`:235`），UI 经 `collectAsStateWithLifecycle` 订阅（`app/.../BeamNavHost.kt:76`）；Rust 回调线程不直接改 UI，只 `StateFlow` 通知。 | 未开始 |
| AU-16 | 性能：解码 T1 ≤8 ms、T2 ≤5 ms（T3 Android 不作为验收目标） | — | 缺失 | 仓库内**无任何验收/基准代码**：`android/` 下无 `androidTest` 目录、无 benchmark 模块（搜 `androidTest`/`benchmark` 零命中），无对 `decodeMs` 的阈值断言或埋点。`SessionStats.decodeMs`（`core-net/.../Models.kt:37`）只是被动展示（`StreamScreen.kt:162`），且该字段由 Rust 侧填充（`NativeBeamBridge.kt:290`），Rust 侧亦无解码耗时统计。T1/T2 口径无法验证。 | 未开始 |
| AU-17 | 里程碑：MVP(列表/配对/解码) | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryScreen.kt:38`；`android/feature-pairing/src/main/java/dev/beam/feature/pairing/PairingScreen.kt:63`；`android/core-decoder/src/main/java/dev/beam/core/decoder/MediaCodecVideoDecoder.kt:29` | 已实现 | 三项功能代码均真实存在：主机列表、配对（扫码+PIN）、MediaCodec 解码器。**但端到端不可用**（AU-01 产物缺失），故本条只就「功能实现」判 `已实现`。 | 未开始 |
| AU-18 | 里程碑：v1(TV、手柄、音频) | `android/app/src/main/java/dev/beam/app/TvMainActivity.kt:15` | 偏差 | 三项均**部分**落地：TV 有入口 Activity 与 LEANBACK_LAUNCHER（`AndroidManifest.xml:58-69`）但无 leanback 依赖（`gradle/libs.versions.toml` 搜 `leanback`/`tv` 零命中）、无 D-pad/焦点导航；手柄有映射（`core-input/.../InputMapper.kt:107-119`）但无采集/回传（PP-03）；音频有 Opus 解码 + AudioTrack（`core-audio/.../LowLatencyAudioPlayer.kt:136`）但非 AAudio（AN-45），且 `BeamSettings.audioEnabled` 默认 `false`（`core-data/.../BeamSettings.kt:26`）。 | 未开始 |

##### §7 Android 构建段

文档原文（`docs/beam-tech-design.md:272`）：

> - Android:buildType debug/release;flavor `store`/`internal`;`:feature-perf` 只在 debug;cargo-ndk 构建 arm64-v8a(+ x86_64 给模拟器)。

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| AB-01 | buildType debug/release | `android/app/build.gradle.kts:23` | 已实现 | `buildTypes { debug { … } release { … } }`（`:23-39`）：debug `isMinifyEnabled=false` + `PERF_STATS=true`（`:27`），release `isMinifyEnabled=true` + `isShrinkResources=true` + `PERF_STATS=false`（`:30-37`），符合 §3.13 门控。 | 未开始 |
| AB-02 | flavor `store`/`internal` | — | 缺失 | **未找到**。对 `android/` 下全部 `*.gradle.kts`/`*.gradle` 搜索 `flavorDimensions`、`productFlavors`、`flavor` **命中 0**（`app/build.gradle.kts` 全文 104 行，只有 `buildTypes` 无 `flavors {}` 块）。无 `store`/`internal` 两个变体，发布渠道差异无从配置。搜索关键词：`flavorDimensions`、`productFlavors`、`flavor`、`create("store")`、`create("internal")`。 | 已完成 |
| AB-03 | `:feature-perf` 只在 debug | `android/app/build.gradle.kts:86` | 已实现 | 依赖图层面成立：全仓仅 `debugImplementation(project(":feature-perf"))`（`:86`，`:84-85` 注释说明 release 变体不含本行），release 变体用空实现 `app/src/release/java/dev/beam/app/perf/PerfEntry.kt:16-33`。**但** perf 类型仍进 release APK，且 §7:276 的 APK 检查 grep 模式漏匹配（详见 AN-25）。 | 未开始 |
| AB-04 | cargo-ndk 构建 arm64-v8a(+ x86_64 给模拟器) | `android/core-net/README.md:57` | 偏差 | 目录与命令已就位但**构建未接线**：`core-net/src/main/jniLibs/arm64-v8a/` 与 `jniLibs/x86_64/` 存在（各含占位 README，`.gitignore` 忽略 `*.so`），cargo-ndk 命令写在 `core-net/README.md:57-75`；但对 `android/` 全部 `*.gradle.kts` 搜 `cargo`/`ndk`/`abiFilters`/`jniLibs` **命中 0** —— `core-net/build.gradle.kts` 无 Exec 任务、无 `MergeSourceSetFolders` 依赖，README §2.4（`:83-94`）给出的接线片段明确标注为「可选，推荐」且未落地；`app/build.gradle.kts` 也无 `ndk { abiFilters }` 或 splits 配置。产物因此只能手工复制，且当前不存在。 | 已完成 |

##### 产品方案 §7 Android 模块结构（补充）

文档原文（`docs/beam-product-plan.md:73-89`）与 §3.12 的差异项，逐条核对：

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| PP-01 | `:feature-discovery` 主机列表、NsdManager 兜底、**WOL** | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/HostScanner.kt:33` | 缺失 | 主机列表与 NsdManager 兜底已实现（AN-07/AN-08）。**WOL 未实现**：全仓搜 `WOL`/`Wake`/`wake`/`MagicPacket`/`magicPacket` 零命中，无 Wake-on-LAN 魔术包构造或 UDP 广播代码。 | 未开始 |
| PP-02 | `:feature-settings` 串流设置页(画质、帧率、解码器、统计浮层开关) | `android/feature-settings/src/main/java/dev/beam/feature/settings/SettingsScreen.kt:72` | 已实现 | 四项逐一到位：画质（分辨率 `:78` + 帧率 `:82` + 码率 `:88`）、解码器偏好（`:119-128`，`DecoderPreference.AUTO/LOW_LATENCY/SYSTEM`）、统计浮层开关（`:140` `SwitchRow`）；另有编码、FEC、音频、主题与恢复默认。 | 未开始 |
| PP-03 | `:core-input` 触控/**实体手柄/遥控采集**，映射为 InputEvent **回传主机** | `android/core-input/src/main/java/dev/beam/core/input/MotionActions.kt:21` | 偏差 | 映射层已实现（AN-41），但**采集与回传两端都断**：`MotionActions.kt` 仅有 `MotionEvent`→`TouchAction` 的静态映射函数，全仓无任何调用方（`MotionEvent` 仅在 core-input 内出现，无 `pointerInput`/`onTouchEvent` 采集点）；回传侧 `Session.sendInput`（`core-net/.../Session.kt:27`）在 `NativeBeamBridge.kt:525` 有实现，但 `feature-stream/.../StreamViewModel.kt:201` 的 `sendInput` **无调用方**，触控/手柄事件从未上行主机。 | 已完成 |
| PP-04 | `:app` Compose 导航、**全屏沉浸**、TV Leanback 入口 | `android/app/src/main/java/dev/beam/app/MainActivity.kt:15` | 偏差 | Compose 导航已实现（AN-01）、TV Leanback 入口已注册（AN-04）。**「全屏沉浸」仅做到 edge-to-edge**：`MainActivity.kt:15` 与 `TvMainActivity.kt:19` 只调用 `enableEdgeToEdge()`；全仓搜 `immersive`/`WindowInsets`/`systemBars`/`hide(`/`WindowInsetsControllerCompat` 零命中 —— 未隐藏系统栏、未做沉浸式粘性沉浸，「串流页全屏沉浸」未实现。 | 已完成 |

##### 产品方案 §16 设计规范

文档原文（`docs/beam-product-plan.md:216-229`）：

> | 令牌 | 深色 | 浅色 | 用途 |  背景 bg #0b0f17 / #f4f6fb；面板 panel #121826 / #ffffff；分隔 line #222b3d / #dfe4ee；正文 / 次要 #e8edf6 / #8f9ab0 / #151b28 / #5d6880；主色 #5b7cff / #3d5cf5；成功/警告/错误 #22c55e / #f59e0b / #ef4444 / #16a34a / #d97706 / #dc2626；串流浮层 #0a0e18 85%
> - 字体:…标题 18/600,正文 14/400,辅助 12/400,PIN 26/700。
> - 间距 4 的倍数:卡片内边距 14、间距 12、页边距 20;圆角 按钮 8、卡片 12、手机外框 30。
> - 组件:主/次/危险按钮、分段选择器、状态胶囊、设备行、模态弹窗、Toast、加载转圈、PIN 输入格。
> - 状态:空态(图标+说明+主操作)、加载、错误(描红+剩余次数)、断线(遮罩+保留最后一帧)。

| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| DS-01 | 背景 bg `#0b0f17` / `#f4f6fb` | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:26` | 已实现 | `DarkBackground = Color(0xFF0B0F17)`（`:26`）、`LightBackground = Color(0xFFF4F6FB)`（`:52`），并接入 `app/.../theme/BeamTheme.kt:34`/`:54`。 | 未开始 |
| DS-02 | 面板 panel `#121826` / `#ffffff` | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:29` | 已实现 | `DarkPanel = 0xFF121826`（`:29`）、`LightPanel = 0xFFFFFFFF`（`:55`）；接 `BeamTheme.kt:36`/`:56`。 | 未开始 |
| DS-03 | 分隔 line `#222b3d` / `#dfe4ee` | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:35` | 已实现 | `DarkLine = 0xFF222B3D`（`:35`）、`LightLine = 0xFFDFE4EE`（`:62`）；接 `BeamTheme.kt:41-42`/`:61-62`。 | 未开始 |
| DS-04 | 正文 `#e8edf6` / `#151b28` | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:38` | 已实现 | `DarkText = 0xFFE8EDF6`（`:38`）、`LightText = 0xFF151B28`（`:64`）；接 `BeamTheme.kt:35`/`:57`。 | 未开始 |
| DS-05 | 次要 `#8f9ab0` / `#5d6880` | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:41` | 已实现 | `DarkTextSub = 0xFF8F9AB0`（`:41`）、`LightTextSub = 0xFF5D6880`（`:67`）；接 `BeamTheme.kt:39`/`:59`。 | 未开始 |
| DS-06 | 主色 `#5b7cff` / `#3d5cf5` | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:44` | 已实现 | `DarkPrimary = 0xFF5B7CFF`（`:44`）、`LightPrimary = 0xFF3D5CF5`（`:70`）；接 `BeamTheme.kt:26`/`:51`。 | 未开始 |
| DS-07 | 成功/警告/错误 `#22c55e`/`#f59e0b`/`#ef4444` / `#16a34a`/`#d97706`/`#dc2626` | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:78` | 偏差 | **只有一套（深色）值**：`Success = 0xFF22C55E`（`:78`）、`Warning = 0xFFF59E0B`（`:81`）、`Error = 0xFFEF4444`（`:84`）；浅色变体 `#16a34a`/`#d97706`/`#dc2626` 未定义（`BeamTokens.kt` 全文无这三个色值）。后果可见：`app/.../theme/BeamTheme.kt:68` 的 `lightColorScheme(error = BeamTokens.Error)` 在浅色主题里直接用深色错误色 `#EF4444`。 | 已完成 |
| DS-08 | 串流浮层 `#0a0e18` 85% | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:87` | 已实现 | `OverlaySurface = Color(0xD90A0E18)`（`:87`），alpha 0xD9=217/255≈85.1%，与「#0a0e18 85%」一致；用于统计浮层 `StreamScreen.kt:143` 与工具栏 `:209`。 | 未开始 |
| DS-09 | 字体 标题 18/600 | `android/app/src/main/java/dev/beam/app/theme/BeamTheme.kt:79` | 已实现 | `titleLarge = TextStyle(fontWeight = FontWeight.SemiBold, fontSize = BeamTokens.TitleSize.value.sp)`（`:79-84`），`TitleSize = 18.sp`（`BeamTokens.kt:123`）；各页标题实际使用（如 `DiscoveryScreen.kt:60`、`SettingsScreen.kt:66`）。 | 未开始 |
| DS-10 | 字体 正文 14/400 | `android/app/src/main/java/dev/beam/app/theme/BeamTheme.kt:91` | 已实现 | `bodyLarge`/`bodyMedium` = `FontWeight.Normal` + `BeamTokens.BodySize`（`:91-102`），`BodySize = 14.sp`（`BeamTokens.kt:126`）。 | 未开始 |
| DS-11 | 字体 辅助 12/400 | `android/app/src/main/java/dev/beam/app/theme/BeamTheme.kt:103` | 已实现 | `bodySmall`/`labelSmall` = Normal + `BeamTokens.CaptionSize`（`:103-108`、`:115-120`），`CaptionSize = 12.sp`（`BeamTokens.kt:129`）。 | 未开始 |
| DS-12 | 字体 PIN 26/700 | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:132` | 偏差 | 令牌 `PinSize = 26.sp` 已定义（`:132`）但**全仓无引用**（搜 `PinSize` 仅命中定义处）；PIN 输入格实际用 `MaterialTheme.typography.titleLarge`（`feature-pairing/.../PairingScreen.kt:195-198`，即 18/600），且全仓无 `FontWeight.Bold`（搜 `FontWeight.Bold` 零命中），700 字重未落地。 | 已完成 |
| DS-13 | 间距 4 的倍数·卡片内边距 14 | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:104` | 已实现 | `SpacingLg = 14.dp`（`:104`），实际用于卡片内边距：`DiscoveryScreen.kt:131`、`:196`，`PairingScreen.kt:233`，`SettingsScreen.kt:169`。 | 未开始 |
| DS-14 | 间距 4 的倍数·间距 12 | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:100` | 已实现 | `SpacingMd = 12.dp`（`:100`），用于元素间距（如 `DiscoveryScreen.kt:67`、`:98`，`PairingScreen.kt:85`、`:153`）。 | 未开始 |
| DS-15 | 间距 4 的倍数·页边距 20 | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:107` | 已实现 | `SpacingXl = 20.dp`（`:107`），用于页面外边距：`DiscoveryScreen.kt:50`、`PairingScreen.kt:76`、`SettingsScreen.kt:61`、`PerfPanel.kt:50`。 | 未开始 |
| DS-16 | 圆角 按钮 8 | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:112` | 已实现 | `RadiusButton = 8.dp`（`:112`），实际使用 `PairingScreen.kt:190`、`:215`，`SettingsScreen.kt:205`、`:212`，`StreamScreen.kt:209`。 | 未开始 |
| DS-17 | 圆角 卡片 12 | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:115` | 已实现 | `RadiusCard = 12.dp`（`:115`），实际使用 `DiscoveryScreen.kt:127`、`:188`，`PairingScreen.kt:225`、`:263`，`SettingsScreen.kt:165`，`PerfPanel.kt:97`、`:129`、`:176`。 | 未开始 |
| DS-18 | 圆角 手机外框 30 | `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:118` | 已实现 | `RadiusPhone = 30.dp`（`:118`），取值与文档一致（原型专用，App 端无对应组件）。 | 未开始 |
| DS-19 | 组件：主/次/**危险**按钮 | `android/feature-pairing/src/main/java/dev/beam/feature/pairing/PairingScreen.kt:211` | 偏差 | 主按钮有（`Button`，`PairingScreen.kt:211`）、次按钮有（`OutlinedButton`，`feature-perf/.../PerfPanel.kt:75`）；**危险按钮缺失**：全仓搜 `danger`/`Danger`/`危险`/`delete`/`Delete` 零命中，无破坏性操作按钮样式。 | 已完成 |
| DS-20 | 组件：分段选择器 | `android/feature-settings/src/main/java/dev/beam/feature/settings/SettingsScreen.kt:183` | 已实现 | `SegmentedRow` 真实渲染选中态分段控件（`:183-228`，含选中描边/填充 `:203-214`），用于分辨率/帧率/码率/格式/FEC/解码器/主题。 | 未开始 |
| DS-21 | 组件：状态胶囊 | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryScreen.kt:166` | 已实现 | `StatusPill`（`DiscoveryScreen.kt:166-182`，胶囊圆角 99.dp + 状态色「已配对/未配对」）；另有 `feature-perf/.../PerfPanel.kt:212` 的「仅 Debug 构建」胶囊。 | 未开始 |
| DS-22 | 组件：设备行 | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryScreen.kt:118` | 已实现 | `HostCard` 真实渲染主机行（图标+名称+状态胶囊+地址+「已配对/配对」操作，`:118-163`）。 | 未开始 |
| DS-23 | 组件：模态弹窗 | — | 缺失 | **未找到**。全仓搜 `AlertDialog`/`Dialog(`/`ModalBottomSheet` 零命中，无任何模态弹窗组件；所有提示均用内联 `NoticeCard`/`CenterNotice`（`PairingScreen.kt:222`、`StreamScreen.kt:111`）。 | 已完成 |
| DS-24 | 组件：Toast | — | 缺失 | **未找到**。全仓搜 `Toast`/`Toast.`/`Snackbar` 零命中；短提示全部走页面内 `NoticeCard`（`DiscoveryScreen.kt:185`、`PairingScreen.kt:222`）。 | 已完成 |
| DS-25 | 组件：加载转圈 | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryScreen.kt:86` | 已实现 | `CircularProgressIndicator` 真实使用：`DiscoveryScreen.kt:86`（扫描中）、`PairingScreen.kt:120`（配对中）、`StreamScreen.kt:117`（连接/重连中）。 | 未开始 |
| DS-26 | 组件：PIN 输入格 | `android/feature-pairing/src/main/java/dev/beam/feature/pairing/PairingScreen.kt:175` | 已实现 | `PinSlots` 真实渲染 `pinLength` 个输入格并区分填充态（`:175-202`）；圆角用 `RadiusButton`（`:190`）。 | 未开始 |
| DS-27 | 状态：空态(图标+说明+**主操作**) | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryScreen.kt:202` | 偏差 | 空态有图标「🔗」（`:207`）与说明文案（`:211-219`），但**缺主操作按钮**：`EmptyState` 内只有两行 Text，无任何可点击主操作；页面级「刷新」按钮在头部（`DiscoveryScreen.kt:63`），不在空态内。 | 已完成 |
| DS-28 | 状态：加载 | `android/feature-discovery/src/main/java/dev/beam/feature/discovery/DiscoveryScreen.kt:84` | 已实现 | `isScanning` 时显示转圈 + 「正在搜索局域网…」（`:82-96`）；配对中/串流中亦各有加载态（`PairingScreen.kt:118-130`、`StreamScreen.kt:101`）。 | 未开始 |
| DS-29 | 状态：错误(描红+剩余次数) | `android/feature-pairing/src/main/java/dev/beam/feature/pairing/PairingScreen.kt:136` | 已实现 | 描红：`NoticeCard(state.error ?: "配对失败", BeamTokens.Error)`（`:136`，卡片 `border = BorderStroke(1.dp, accent)` `:227`）；剩余次数：`PairingViewModel.kt:178-179` 在 `E3001` 文案中拼接「（剩余 $it 次）」，`E3002` 显示锁定提示（`:180`）。 | 未开始 |
| DS-30 | 状态：断线(遮罩+保留最后一帧) | `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamScreen.kt:102` | 已实现 | 遮罩：`CenterNotice` 覆盖半透明黑 `Color(0xCC000000)`（`:113`）+ 居中转圈；文案「连接中断，正在重连…（保留最后一帧）」（`:102`），与实现一致 —— `StreamViewModel.kt:219-224` 进入 RECONNECTING 时不停解码器、不释放 SurfaceView，最后一帧得以保留。（遮罩底色用 `0xCC000000` 而非 §16 的 `OverlaySurface`，属遮罩与浮层两套语义，不判为偏差。） | 未开始 |
| DS-31 | 字体：Android Noto Sans CJK | `android/app/src/main/java/dev/beam/app/theme/BeamTheme.kt:81` | 已实现 | 全部 `TextStyle` 用 `FontFamily.SansSerif`（`:81/87/93/99/105/111/117`），Android 系统 sans-serif 的 CJK 字形即 Noto Sans CJK；源码注释 `BeamTheme.kt:21-23` 明示该等价关系。未打包独立字体资源。 | 未开始 |

---

#### 文档未覆盖但代码里有的

以下为文档未要求、但仓库中存在且已造成偏差/风险的实现，逐条标注 `超出`：

1. **超出** — `android/core-audio/src/main/java/dev/beam/core/audio/PcmRingBuffer.kt:14`：完整的单生产者单消费者 PCM 环形缓冲（有单测 `PcmRingBufferTest.kt`），但 `LowLatencyAudioPlayer` 走同步解码直写 `AudioTrack`（`:168`），**全仓无调用方**（搜 `PcmRingBuffer` 仅命中定义与测试）。文档 §3.12 未要求该结构，属未接线的额外实现，且掩盖了「解码线程与输出线程未分离」的真实延迟风险。
2. **超出** — `android/core-renderer/src/main/java/dev/beam/core/renderer/SurfaceFrameRate.kt:51`：`fun interface SurfaceSource` 无人实现、无使用方；是「Surface 管理」职责未落到本模块的证据（见 AN-36）。
3. **超出** — `android/core-net/src/main/java/dev/beam/core/net/Perf.kt:32`：`interface PerfTickSource` 无任何实现类，`:13` 的 `PerfTick` 亦无生产者；`PerfRecorder.kt:31` 因此默认 `emptyFlow()`，性能面板永远拿不到阶段耗时（见 AN-23）。文档 §3.12 未描述 Kotlin 侧 `perf://tick` 契约。
4. **超出** — `android/core-net/src/main/java/dev/beam/core/net/StreamCfg.kt:32`：`Tier` 枚举（T0/T1/T2）+ `StreamCfg.tier` 把 §8 的验收档位引入 Android 协议模型，但全仓无使用方（搜 `tier` 仅命中定义处），且会随分辨率误映射档位（如 2560 宽判为 T2）。文档 §3.12 未要求该字段。
5. **超出** — `android/core-data/src/main/java/dev/beam/core/data/ui/BeamTokens.kt:21`：设计令牌放在 `:core-data`（依赖图上位于 UI 层之下），是因 §3.12 的 12 模块清单里没有 `:core-ui` 而做的替代（源码注释 `:10-16` 已说明）。本身合理，但它使 `:core-data` 同时承载「设置持久化」与「设计系统」两类职责，是后续分层的隐性债务。

---

#### 最该优先补的（最多 3 条）

1. **打通 uniffi 产物链路，否则 Android 串流整体不可用**（AU-01、AB-04、AU-13）。`crates/beam-ffi` 目前**没有任何 uniffi 导出面**（无 `setup_scaffolding!`/`#[uniffi::export]`、无 `.udl`、无 `crate-type=["cdylib"]`），`libbeam_ffi.so` 与生成的 Kotlin 绑定均不存在，`android/` 内也没有 cargo-ndk 任务。`NativeBeamBridge` 的反射层写得再完整也永远只能落到 `UnavailableBeamClient`。最小修复顺序：① `crates/beam-ffi` 加 uniffi 导出（UDL 或 proc-macro）+ `crate-type = ["cdylib"]`；② `core-net/build.gradle.kts` 按 `core-net/README.md:83-94` 落 Exec 任务产出 `jniLibs/{arm64-v8a,x86_64}/libbeam_ffi.so`；③ 在 so 内起 tokio 运行时（AU-13）。
2. **补齐「会话可用」的最后一公里：输入采集→回传 + 500 ms 统计节拍**（PP-03、AU-10）。`InputMapper` 与 `MotionActions` 已就绪但无人调用，`StreamViewModel.sendInput`（`:201`）也无调用方 —— 触控/手柄/遥控事件从未上行主机；`session://stats` 的 500 ms 节拍在 Kotlin 与 Rust 两侧都没有产生者，`stats` 流恒空，统计浮层与性能面板拿不到数据。修这两条后「能连上」才等于「能用」。
3. **修复性能门禁与性能面板的可验证性**（AN-25、AN-23、AU-16）。`:core-data`/`:core-net` 的 perf 类型在 main 源集，会进 release APK，而 CI 的 `grep -qi "feature-perf\|perfstats"`（`.github/workflows/ci.yml:103`）匹配不到 `dev/beam/core/data/perf/*`，§7:276 的「release 无 perf 类」门禁形同虚设；同时 `PerfRecorder` 无调用方、`Routes.PERF` 无入口、解码 T1/T2 无任何验收代码。建议把 perf 模型整体移入 debug-only 源集、把 CI grep 改为 `dev/beam/core/data/perf\|dev/beam/core/net/Perf\|feature-perf`，并补一条解码耗时的测量与阈值断言。

---

#### 附：本次审计的核对方式与已知边界

- 全部结论基于**源码与构建脚本静态阅读**；未运行 gradle / npm / cargo（只读约束），未修改仓库内任何文件。
- 搜索关键词（用于「缺失」判定的证据）：`flavorDimensions`、`productFlavors`、`flavor`、`cargo`、`ndk`、`abiFilters`、`jniLibs`、`System.loadLibrary`、`Class.forName`、`InputMapper`、`sendInput`、`PerfTickSource`、`PerfRecorder`、`PcmRingBuffer`、`WOL`/`Wake`/`MagicPacket`、`AlertDialog`/`Dialog(`/`ModalBottomSheet`、`Toast`/`Snackbar`、`danger`/`危险`、`FontWeight.Bold`、`PinSize`、`RadiusPhone`、`immersive`/`WindowInsets`/`systemBars`、`KeyStore`/`Cipher`、`androidTest`/`benchmark`、`500`/`delay(500)`/`ticker`、`uniffi::`/`setup_scaffolding`/`derive(Uniffi)`、`*.udl`、`tokio`。
- 范围边界：本次只审计 Android 侧 12 个模块与对应文档章节；`crates/*` 仅在能直接证明 Android 侧条目时作为旁证引用（AU-01、AU-13）。
- 已复核的既知事实：uniffi 绑定与 `libbeam_ffi.so` 尚未生成（AU-01，本次额外发现 Rust 侧连导出面都未建立）；`:core-audio` 未使用 AAudio 而改用 `AudioTrack`（AN-45，源码与 README 均有明示）。
### 9. Tauri 桌面端（§3.11 / §2 / 产品方案 §6 §15 §16）

> 来自 `part-6.md`

#### 条目

##### §3.11 Commands
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| CM-01 | `get_host_status` | `apps/desktop/src-tauri/src/commands.rs:30-33`、`apps/desktop/src-tauri/src/main.rs:76`、`apps/desktop/src-tauri/src/service_crates.rs:273-275` | 已实现 | 已注册；返回真实 `HostStatus`（真实主机名、真实本机 IPv4、真实端口与生效配置）。`gpu` 恒 `None`（无采集后端探测），但文档未要求该字段取值。 | 未开始 |
| CM-02 | `start_hosting(opts)` | `apps/desktop/src-tauri/src/commands.rs:38-48`、`apps/desktop/src-tauri/src/service_crates.rs:277-359` | 偏差 | 命令存在、签名正确、会广播 `host://state-changed` 并同步托盘（`apps/desktop/src-tauri/src/commands.rs:45-46`）；但 crates 路径只做了配置合并校验 + 真实 mDNS 广播（`apps/desktop/src-tauri/src/service_crates.rs:310-353`），**没有启动任何主机会话/采集/编码/QUIC 监听**，即「已开播」的主机不会真的接受客户端接入。`host_stream_config()` 这个唯一调用 `beam-host` 的封装是死代码（`apps/desktop/src-tauri/src/service_crates.rs:254-256`，无调用点）。 | 已完成 |
| CM-03 | `stop_hosting` | `apps/desktop/src-tauri/src/commands.rs:53-59`、`apps/desktop/src-tauri/src/service_crates.rs:361-376` | 已实现 | 真实停止 mDNS 广播（`advertiser = None`）、状态归 `Idle`、广播事件并同步托盘。 | 未开始 |
| CM-04 | `get_pairing_code` | `apps/desktop/src-tauri/src/commands.rs:68-71`、`apps/desktop/src-tauri/src/service_crates.rs:378-381`、`apps/desktop/src-tauri/src/service_crates.rs:890-919` | 已实现 | crates 路径真实：`Identity` 自签证书指纹 + `PairingToken::issue(120s)` + `beam://pair?h=..&fp=..&t=..` 载荷。LocalService 路径为合成 PIN/指纹/token（`apps/desktop/src-tauri/src/service.rs:764-778`）。 | 未开始 |
| CM-05 | `approve_pair(id)` | `apps/desktop/src-tauri/src/commands.rs:74-79`、`apps/desktop/src-tauri/src/service_crates.rs:383-417` | 已实现 | crates 路径真实写入信任库并 `save` 落盘（`apps/desktop/src-tauri/src/service_crates.rs:411-413`）。**但** `pending_pairs` 从未被写入（`apps/desktop/src-tauri/src/service_crates.rs:238` 初始化为空后只有 `retain`/`iter`），所以设备名与平台必然回落为「未知设备」「未知平台」（`apps/desktop/src-tauri/src/service_crates.rs:394-401`）——落库是真的，来路是假的。 | 未开始 |
| CM-06 | `reject_pair(id)` | `apps/desktop/src-tauri/src/commands.rs:82-87`、`apps/desktop/src-tauri/src/service_crates.rs:419-430` | 偏差 | crates 路径下 `pending_pairs` 恒空 → `retain` 长度不变 → 任何 id 都返回 `E9101`「配对请求不存在」（`apps/desktop/src-tauri/src/service_crates.rs:423-427`）。命令在真实后端上不可用。 | 已完成 |
| CM-07 | `remove_device(id)` | `apps/desktop/src-tauri/src/commands.rs:90-95`、`apps/desktop/src-tauri/src/service_crates.rs:432-449` | 已实现 | 真实从信任库移除并写回文件（`apps/desktop/src-tauri/src/service_crates.rs:440-447`）。 | 未开始 |
| CM-08 | `list_hosts` | `apps/desktop/src-tauri/src/commands.rs:102-105`、`apps/desktop/src-tauri/src/service_crates.rs:460-482`、`apps/desktop/src-tauri/src/service_crates.rs:1033-1057` | 已实现 | crates 路径为真实 mDNS 浏览（独立线程驱动 `Browser::start()` 流，`Found`/`Lost` 增删）；`trusted` 由信任库指纹比对得出。LocalService 路径为 2 台合成主机（`apps/desktop/src-tauri/src/service.rs:887-910`）。 | 未开始 |
| CM-09 | `connect(host_id)` | `apps/desktop/src-tauri/src/commands.rs:108-111`、`apps/desktop/src-tauri/src/service_crates.rs:484-551`、`apps/desktop/src-tauri/src/transport.rs:42-73` | 偏差 | 命令与状态机推进是真的（`Discovering→Pairing→Connecting→Negotiating`，`apps/desktop/src-tauri/src/service_crates.rs:514-524`），QUIC/TLS 握手也是真的（`apps/desktop/src-tauri/src/transport.rs:62-69`）；但握手成功后这条连接**立即被关闭**（`apps/desktop/src-tauri/src/transport.rs:72` `conn.close(0, b"probe")`，注释自述「只用于探测」），没有帧传输、没有解码/渲染通道，语义上不构成「建立一次串流会话」。 | 已完成 |
| CM-10 | `disconnect` | `apps/desktop/src-tauri/src/commands.rs:116-129`、`apps/desktop/src-tauri/src/service_crates.rs:553-561` | 已实现 | 结束会话、状态机 → `Closed`，并广播 `session://disconnected`（`reason="by_user"`），载荷结构正确（`apps/desktop/src-tauri/src/dto.rs:235-240`）。 | 未开始 |
| CM-11 | `get_settings` | `apps/desktop/src-tauri/src/commands.rs:136-139`、`apps/desktop/src-tauri/src/service_crates.rs:599-602` | 已实现 | crates 路径返回 `beam-config` 合并后的真实配置 + 桌面端自有项。 | 未开始 |
| CM-12 | `set_settings(patch)` | `apps/desktop/src-tauri/src/commands.rs:144-153`、`apps/desktop/src-tauri/src/service_crates.rs:604-742` | 已实现 | 增量 patch → `merge` → `validate` → 真实落盘 `config.toml` 与 `desktop.json`（`apps/desktop/src-tauri/src/service_crates.rs:705-725`），非法值返回 `E11001`。 | 未开始 |
| CM-13 | debug 下另有 `perf_snapshot` | `apps/desktop/src-tauri/src/commands.rs:160-164`（debug）/ `apps/desktop/src-tauri/src/commands.rs:168-175`（release） | 占位 | 命令与 `#[cfg(debug_assertions)]` 门控正确，release 版本返回 `E11003`（符合 §3.13 门控）；但实现体是桩：crates 路径返回 `available:false` + 空 stages/metrics（`apps/desktop/src-tauri/src/service_crates.rs:744-753`），LocalService 路径返回 8 个固定假值与 8 个固定指标（`apps/desktop/src-tauri/src/service.rs:558-628`）。`beam-perf` crate 存在（`crates/beam-perf/Cargo.toml`）但**未进入桌面端依赖**（`apps/desktop/src-tauri/Cargo.toml:57-69`）。 | 已完成 |
| CM-14 | debug 下另有 `perf_export(format)` | `apps/desktop/src-tauri/src/commands.rs:178-182`（debug）/ `apps/desktop/src-tauri/src/commands.rs:185-192`（release） | 占位 | 门控同上；crates 路径恒返回 `E11003`「未接入桌面端」（`apps/desktop/src-tauri/src/service_crates.rs:759-766`）；LocalService 路径虽真实写文件（`apps/desktop/src-tauri/src/service.rs:644-667`），但导出的内容是合成假数据。 | 已完成 |
| CM-15 | （文档清单外）`list_devices` | `apps/desktop/src-tauri/src/commands.rs:202-205`、`apps/desktop/src-tauri/src/main.rs:95` | 超出 | 文档 §3.11 的 12 个 command 里没有读取已配对设备列表的入口，代码补了一个纯读取 command（`apps/desktop/src-tauri/src/main.rs:94` 注释自述「补充:『设备』页所需的只读入口」）。不改状态，风险低，但属文档未列出的接口面。 | 未开始 |
| CM-16 | （文档清单外）`list_pair_requests` | `apps/desktop/src-tauri/src/commands.rs:210-213`、`apps/desktop/src-tauri/src/main.rs:96` | 超出 | 同上，为「设备」页补的只读入口。 | 未开始 |

##### §3.11 职责 / 托盘 / 错误
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| CM-17 | 职责:UI | `apps/desktop/src/App.tsx:35-113`、`apps/desktop/src/pages/HomePage.tsx:25` | 已实现 | 5 个页面 + 侧栏导航 + 全局事件订阅，页面只读 store、不直接碰 IPC（`apps/desktop/src/App.tsx:32-33`）。 | 未开始 |
| CM-18 | 职责:托盘 | `apps/desktop/src-tauri/src/tray.rs:27-61`、`apps/desktop/src-tauri/src/tray.rs:76-113` | 已实现 | 托盘图标 + 4 个菜单项齐全：「▶ 一键开播」（`apps/desktop/src-tauri/src/tray.rs:28`）、「■ 停止串流」（`apps/desktop/src-tauri/src/tray.rs:29`）、「显示主窗口」（`apps/desktop/src-tauri/src/tray.rs:30`、`apps/desktop/src-tauri/src/tray.rs:105-110`）、「退出 Beam」（`apps/desktop/src-tauri/src/tray.rs:31`、`apps/desktop/src-tauri/src/tray.rs:111`）；开播/停止复用同一套服务状态并回广播 `host://state-changed`。 | 未开始 |
| CM-19 | 职责:通知 | 未找到（已搜索：`notification`、`Notification`、`通知` 于 `apps/desktop/src-tauri/src/**`、`apps/desktop/src/**`） | 缺失 | 全仓只有文档性提及（`apps/desktop/src-tauri/src/main.rs:4`、`apps/desktop/README.md:6`），没有任何系统通知实现（无 `tauri-plugin-notification`、无 `NotificationExt`、无 Windows toast 调用）。`Cargo.toml` 也未引入通知插件（`apps/desktop/src-tauri/Cargo.toml:35-42`）。 | 未开始 |
| CM-20 | 只通过 commands/events 与 Rust 交互 | `apps/desktop/src/ipc/commands.ts:29-35`、`apps/desktop/src/ipc/events.ts:21-38`、`apps/desktop/src-tauri/capabilities/default.json:6` | 已实现 | 前端 Rust 交互只有 `invoke` 与 `listen` 两个入口（`apps/desktop/src/ipc/commands.ts:11`、`apps/desktop/src/ipc/events.ts:8`）；权限只开 `core:default`，未开文件系统/Shell（`apps/desktop/src-tauri/capabilities/default.json:6`）。 | 未开始 |
| CM-21 | 视频帧不经 IPC | `apps/desktop/src-tauri/src/dto.rs:3-4`、`apps/desktop/src/ipc/events.ts:21-38`、`apps/desktop/src-tauri/src/events.rs:24-31` | 已实现 | 8 个事件的载荷全是控制面/统计面结构（`apps/desktop/src-tauri/src/dto.rs:199-240`），没有任何帧/像素/缓冲区类型；前端亦无帧通道（已搜索 `frame`、`pixel`、`buffer`、`video` 于 `apps/desktop/src/**`）。**符合**文档要求。 | 未开始 |
| CM-22 | 错误:command 返回 `{code:"E2001", message, retryable}` | `apps/desktop/src-tauri/src/error.rs:56-65` | 已实现 | `BeamError` 三个字段与文档逐字一致，`#[serde(rename_all = "camelCase")]` 不改变这三个键名。 | 未开始 |
| CM-23 | 前端按 code 显示中文文案 | `apps/desktop/src/ipc/errors.ts:27-104` | 已实现 | `ERROR_TEXT` 按码查表，`errorText()` 命中即返回中文文案，未登记码回退兜底文案并附后端 message（`apps/desktop/src/ipc/errors.ts:99-104`）；`toBeamError` 兼容 Tauri 把 `Err` 序列化成 JSON 字符串的情况（`apps/desktop/src/ipc/errors.ts:68-88`）。 | 已完成 |
| CM-24 | 文案表覆盖后端会返回的错误码 | `apps/desktop/src/ipc/errors.ts:27-62`、`apps/desktop/src-tauri/src/error.rs:14-53` | 缺失 | 前端表覆盖了 `error.rs` 自己定义的 25 个码，但**没有覆盖 crates 侧会真实透传过来的码**：`transport.rs` 把 `beam-net` 的 `err.code_str()` 直接当作 code 上报（`apps/desktop/src-tauri/src/transport.rs:64-69`），而 `beam-net` 定义了 `E2005`/`E2006`/`E2007`/`E2008`（`crates/beam-net/src/error.rs:15-19`、`crates/beam-net/src/error.rs:88-89`，`E2007` 正是 `apps/desktop/src-tauri/src/transport.rs:53` 注释提到的缺 TLS 身份错误）；`beam-input` 的 `E8001`/`E8002`（`crates/beam-input/src/error.rs:29`）、`beam-decode` 的 `E10001`（`crates/beam-decode/src/error.rs:35`）、`beam-render` 的 `E10011`–`E10014`（`crates/beam-render/src/error.rs:36-51`）同样无文案。命中的码一律落到兜底文案「操作失败,请稍后重试」。 | 已完成 |

##### §3.11 Events
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| EV-01 | `host://state-changed` | `apps/desktop/src-tauri/src/events.rs:24`、`apps/desktop/src-tauri/src/commands.rs:45`、`apps/desktop/src/ipc/events.ts:23` | 已实现 | 事件名与文档**逐字一致**（含 `://` 分隔符）；载荷 `HostStatus` 前后端类型一致（`apps/desktop/src-tauri/src/dto.rs:41-65` ↔ `apps/desktop/src/ipc/types.ts:14-31`）；在开播/停止/同意/拒绝/移除/改设置时发出，频次为「状态变化时」。 | 未开始 |
| EV-02 | `pair://request` | `apps/desktop/src-tauri/src/events.rs:25`、`apps/desktop/src-tauri/src/events.rs:127-137`、`apps/desktop/src/ipc/events.ts:25` | 偏差 | 事件名逐字一致，载荷 `PairRequest` 类型正确；但发射点只有**启动后 600 ms 推一次现存请求**（`apps/desktop/src-tauri/src/events.rs:131-136`），没有任何「收到新配对请求时」的推送路径；且 crates 路径 `pending_pairs` 恒空（T-05/T-06），所以真实后端上该事件永远不会带来内容。前端监听名一致（`apps/desktop/src/ipc/events.ts:25`）。 | 已完成 |
| EV-03 | `pair://code-rotated` | `apps/desktop/src-tauri/src/events.rs:26`、`apps/desktop/src-tauri/src/events.rs:86-98`、`apps/desktop/src-tauri/src/service_crates.rs:768-778` | 已实现 | 事件名逐字一致；5 s 检查一次、TTL 120 s 到期后真实重算配对码并推送（`apps/desktop/src-tauri/src/events.rs:92-95`），crates 路径的轮换会重新签发真实 token 与真实指纹载荷。载荷 `PairingCode` 类型一致。 | 未开始 |
| EV-04 | `discovery://updated` | `apps/desktop/src-tauri/src/events.rs:27`、`apps/desktop/src-tauri/src/events.rs:100-110`、`apps/desktop/src/ipc/events.ts:29` | 已实现 | 事件名逐字一致；每 5 s 刷新一次并推送 `Vec<DiscoveredHost>`，crates 路径取自真实 mDNS 浏览快照（`apps/desktop/src-tauri/src/service_crates.rs:460-482`）。 | 未开始 |
| EV-05 | `session://stats`（500 ms） | `apps/desktop/src-tauri/src/events.rs:28`、`apps/desktop/src-tauri/src/events.rs:34`、`apps/desktop/src-tauri/src/events.rs:57`、`apps/desktop/src-tauri/src/events.rs:79-81` | 已实现 | 事件名逐字一致；频次常量 `SESSION_STATS_INTERVAL_MS = 500` 与文档一致（`apps/desktop/src-tauri/src/events.rs:34`），定时器每拍发一次；载荷 `SessionStats` 结构与前端 `SessionStats` 一一对应（`apps/desktop/src/ipc/types.ts:107-125`）。**数据来源需注意**：crates 路径各项耗时全为 0（未采集，`apps/desktop/src-tauri/src/service_crates.rs:567-591`），LocalService 路径为正弦波合成值（`apps/desktop/src-tauri/src/service.rs:422-457`）——事件机制是真的，数值不是实测。 | 已完成 |
| EV-06 | `session://reconnecting` | `apps/desktop/src-tauri/src/events.rs:29`、`apps/desktop/src-tauri/src/events.rs:58-61`、`apps/desktop/src-tauri/src/service_crates.rs:593-597` | 偏差 | 事件名逐字一致、载荷 `ReconnectingInfo` 结构正确；但 crates 路径的 `reconnect_notice()` 恒返回 `None`（注释自述「当前 `beam-session` 的帧传输封装尚未落地，这里恒为 `None`」），因此在默认构建下该事件永不触发。LocalService 路径靠 20 s 后模拟一次抖动来触发（`apps/desktop/src-tauri/src/service.rs:398-420`）。 | 未开始 |
| EV-07 | `session://disconnected` | `apps/desktop/src-tauri/src/events.rs:30`、`apps/desktop/src-tauri/src/commands.rs:119-127`、`apps/desktop/src-tauri/src/events.rs:64-77` | 已实现 | 事件名逐字一致；两条发射路径都真实：用户主动断开（`apps/desktop/src-tauri/src/commands.rs:119-127`）与真实传输层握手失败（`apps/desktop/src-tauri/src/events.rs:64-77`，带错误码 `outcome.code`）。载荷 `DisconnectedInfo` 与后端 DTO 一致（`apps/desktop/src-tauri/src/dto.rs:235-240`）。小瑕疵：前端把该载荷类型内联写了一份（`apps/desktop/src/ipc/events.ts:80`），未复用 `apps/desktop/src/ipc/types.ts:133` 的同名接口。 | 未开始 |
| EV-08 | `perf://tick`（debug，250 ms） | `apps/desktop/src-tauri/src/events.rs:31`、`apps/desktop/src-tauri/src/events.rs:36`、`apps/desktop/src-tauri/src/events.rs:113-125`、`apps/desktop/src-tauri/src/service_crates.rs:755-757` | 偏差 | 事件名逐字一致；频次常量 `PERF_TICK_INTERVAL_MS = 250` 与文档一致（`apps/desktop/src-tauri/src/events.rs:36`），且整块包在 `#[cfg(debug_assertions)]` 里（`apps/desktop/src-tauri/src/events.rs:113`）——门控正确。但 crates 路径 `perf_tick()` 恒返回 `None`（`apps/desktop/src-tauri/src/service_crates.rs:755-757`），默认构建下事件永不发出；只有 LocalService 路径会发合成值（`apps/desktop/src-tauri/src/service.rs:630-642`）。 | 已完成 |

##### §2 结构 / 产品方案 §6
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| ST-01 | `src/pages/` 应有 (Home, Devices, Settings, Connect, Perf) 五个页面 | `apps/desktop/src/pages/HomePage.tsx:25`、`apps/desktop/src/pages/DevicesPage.tsx:18`、`apps/desktop/src/pages/SettingsPage.tsx:25`、`apps/desktop/src/pages/ConnectPage.tsx:19`、`apps/desktop/src/pages/PerfPage.tsx:18`、`apps/desktop/src/App.tsx:105-109` | 已实现 | 5 个页面文件齐全，命名与文档一致，且全部在路由中挂上（`apps/desktop/src/App.tsx:105-109`）。 | 未开始 |
| ST-02 | `src/components/` 存在且有内容 | `apps/desktop/src/components/BackendNotice.tsx:1`、`apps/desktop/src/components/Card.tsx:4`、`apps/desktop/src/components/Charts.tsx:1`、`apps/desktop/src/components/DeviceRows.tsx:14`、`apps/desktop/src/components/Feedback.tsx:2`、`apps/desktop/src/components/LatencyOverlay.tsx:6`、`apps/desktop/src/components/PairingPanel.tsx:14`、`apps/desktop/src/components/QrCode.tsx:16`、`apps/desktop/src/components/SegmentedControl.tsx:7`、`apps/desktop/src/components/Sidebar.tsx:14`、`apps/desktop/src/components/StatusPill.tsx:6`、`apps/desktop/src/components/ToastHost.tsx:6`、`apps/desktop/src/components/Toggle.tsx:2` | 已实现 | 13 个组件文件，非空壳。 | 未开始 |
| ST-03 | `src/ipc/` 存在且有内容 | `apps/desktop/src/ipc/commands.ts:37`、`apps/desktop/src/ipc/events.ts:21`、`apps/desktop/src/ipc/errors.ts:27`、`apps/desktop/src/ipc/types.ts:9`、`apps/desktop/src/ipc/index.ts:1` | 已实现 | 5 个文件，command/event/错误/类型分层完整。 | 未开始 |
| ST-04 | `src/stores/` 存在且有内容 | `apps/desktop/src/stores/hostStore.ts:31`、`apps/desktop/src/stores/deviceStore.ts:43`、`apps/desktop/src/stores/sessionStore.ts:30`、`apps/desktop/src/stores/settingsStore.ts:22`、`apps/desktop/src/stores/perfStore.ts:27`、`apps/desktop/src/stores/toastStore.ts:25` | 已实现 | 6 个 store + 1 个 `index.ts` 汇总导出。 | 未开始 |
| ST-05 | `src/i18n/` 存在且有内容 | `apps/desktop/src/i18n/index.tsx:19`、`apps/desktop/src/i18n/zh-CN.ts:7`、`apps/desktop/src/i18n/en.ts:9` | 已实现 | provider + 两份词表。 | 未开始 |
| ST-06 | `src-tauri/` 应有 (commands.rs, events.rs, tray.rs, main.rs) | `apps/desktop/src-tauri/src/commands.rs:1`、`apps/desktop/src-tauri/src/events.rs:1`、`apps/desktop/src-tauri/src/tray.rs:1`、`apps/desktop/src-tauri/src/main.rs:1` | 已实现 | 4 个文件齐全（另有 `dto.rs`/`error.rs`/`service.rs`/`service_crates.rs`/`transport.rs` 5 个支撑文件）。 | 未开始 |
| ST-07 | Windows 作为客户端：用独立原生 D3D11/wgpu 窗口渲染，不走 WebView（产品方案第 45 行） | 未找到（已搜索：`wgpu`、`d3d11`、`D3D11`、`WebviewWindow`、`WebviewBuilder`、`native_window`、`beam_render`、`beam_decode` 于 `apps/desktop/src-tauri/src/**`） | 缺失 | 桌面端只有一个 WebView 窗口（`apps/desktop/src-tauri/tauri.conf.json:14-27`），客户端画面区是纯 CSS 渐变占位加一个「GAME」字样（`apps/desktop/src/pages/ConnectPage.tsx:82`、`apps/desktop/src/styles.css:733-749`）；`beam-render` / `beam-decode` 两个 crate **未进入桌面端依赖**（`apps/desktop/src-tauri/Cargo.toml:57-69`），`beam-render` 自己也标注未启用 `d3d11` feature 时 `present` 返回 `E10014`（`crates/beam-render/src/renderer.rs:45`）。原生渲染窗口这一条整条未落地。 | 已完成 |
| ST-08 | §6 第 69 行 Commands 清单（`get_host_status`、`start_hosting(opts)`/`stop_hosting`、`get_pairing_code`、`approve_pair(id)`/`remove_device(id)`、`list_hosts`/`connect(host_id)`、`get_settings`/`set_settings(patch)`） | `apps/desktop/src-tauri/src/main.rs:74-97`、`apps/desktop/src-tauri/src/commands.rs:30-153` | 已实现 | §6 清单里的 11 个命令全部注册且实现；§3.11 多出的 `reject_pair`/`disconnect` 也已实现（`apps/desktop/src-tauri/src/commands.rs:82-87`、`apps/desktop/src-tauri/src/commands.rs:116-129`）。（注：§6 第 69 行本身比 §3.11 少列 `reject_pair` 与 `disconnect`，是文档两处清单不一致，代码按 §3.11 的更全清单实现。） | 未开始 |
| ST-09 | §6 第 70 行 Events 清单 | `apps/desktop/src-tauri/src/events.rs:24-31`、`apps/desktop/src/ipc/events.ts:23-37` | 已实现 | 6 个事件名与 §6 逐字一致（§6 把 `reconnecting` 写成 `session://disconnected/reconnecting`，代码按 §3.11 的 `session://reconnecting` 实现）；另加 §3.11 的 `session://reconnecting` 与 `perf://tick` 共 8 个，前后端事件名两两对齐。 | 未开始 |
| ST-10 | §6 第 71 行：视频帧永远不经过 IPC | `apps/desktop/src-tauri/src/dto.rs:3-4`、`apps/desktop/src-tauri/src/events.rs:43-47` | 已实现 | 同 T-21：全部事件载荷都是控制面/统计面结构，无帧类型。 | 未开始 |
| ST-11 | §6 crate 表：apps/desktop 应接到 `beam-host`/`beam-client` 等 crate | `apps/desktop/src-tauri/Cargo.toml:57-69`、`apps/desktop/src-tauri/src/service_crates.rs:29-41` | 偏差 | 已接 9 个（beam-host / beam-client / beam-session / beam-config / beam-log / beam-discovery / beam-pair / beam-net / beam-proto），接线是**真实调用**而非声明（`apps/desktop/src-tauri/src/service_crates.rs:29-41` 的 use 列表）。但 §6 表里与桌面端相关的 `beam-decode`、`beam-render`、`beam-audio`、`beam-input`、`beam-perf` 一个都没接（`beam-ffi`/`beam-gamestream` 属 Android 与 v1，不算缺口）——这直接导致 ST-07（无画面）、T-13/T-14（perf 为桩）、EV-08（perf 事件无数据）。 | 未开始 |
| ST-12 | §6 第 67 行：`apps/desktop` = Tauri 2 App | `apps/desktop/package.json:19`、`apps/desktop/package.json:25`、`apps/desktop/src-tauri/Cargo.toml:36`、`apps/desktop/src-tauri/tauri.conf.json:2` | 已实现 | Tauri 2（`@tauri-apps/api ~2.11.1` / `cli ~2.11.5` / Rust `tauri ~2.11.6`），配置与能力文件齐备。 | 未开始 |

##### 产品方案 §15 界面原型（Windows 端功能点）
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| UX-01 | Windows 端可开播/停止 | `apps/desktop/src/pages/HomePage.tsx:111-119`、`apps/desktop/src/stores/hostStore.ts:49-75` | 已实现 | 首页主按钮在「开播/停止」间切换（`apps/desktop/src/pages/HomePage.tsx:112` 与 `:116`），调用真实 `start_hosting`/`stop_hosting` 并弹 Toast。注：开播后的真实能力边界见 T-02。 | 未开始 |
| UX-02 | 弹出二维码与 PIN 配对 | `apps/desktop/src/components/PairingPanel.tsx:43-63`、`apps/desktop/src/components/QrCode.tsx:16-68` | 已实现 | 二维码由真实 `qrcode` 库渲染 `beam://pair?h=..&fp=..&t=..` 载荷（`apps/desktop/src/components/QrCode.tsx:28`），PIN 按 4 位分格展示并显示剩余秒数倒数（`apps/desktop/src/components/PairingPanel.tsx:50-56`）。 | 未开始 |
| UX-03 | 移除设备（空态） | `apps/desktop/src/pages/DevicesPage.tsx:53-60`、`apps/desktop/src/pages/DevicesPage.tsx:71-72`、`apps/desktop/src/pages/DevicesPage.tsx:93-100` | 已实现 | 每行有「移除」按钮（`apps/desktop/src/pages/DevicesPage.tsx:93-100`），走真实 `remove_device`；无设备时显示空态组件。 | 未开始 |
| UX-04 | 切换画质/编码/网络 | `apps/desktop/src/pages/SettingsPage.tsx:60-75`、`apps/desktop/src/pages/SettingsPage.tsx:105-125`、`apps/desktop/src/pages/SettingsPage.tsx:156-175` | 已实现 | 画质（分辨率/帧率/HDR/匹配客户端）、编码（编码器/格式/预设/抓屏方式）、网络（FEC/发现/Tailscale）三组控件齐全，改动即时 `set_settings(patch)`。 | 未开始 |
| UX-05 | 开关延迟浮层 | `apps/desktop/src/pages/SettingsPage.tsx:229-235`、`apps/desktop/src/pages/ConnectPage.tsx:89-97` | 偏差 | 设置项 `showStatsOverlay` 存在且能写入后端（`apps/desktop/src/pages/SettingsPage.tsx:229-235`），但渲染侧**不读这个开关**：`LatencyOverlay` 只要有 stats 就无条件渲染（`apps/desktop/src/pages/ConnectPage.tsx:89-97`），关掉开关界面照旧显示浮层。 | 已完成 |
| UX-06 | 查看 Debug 性能面板 | `apps/desktop/src/pages/PerfPage.tsx:18-125`、`apps/desktop/src/components/Sidebar.tsx:9` | 偏差 | 性能页存在（快照 / E2E 曲线 / 实时指标 / CSV·JSON 导出按钮齐全）。但产品方案第 154 行要求「隐藏入口仅 debug 构建存在」，而侧栏的「诊断」入口在 release 构建里同样常驻（`apps/desktop/src/components/Sidebar.tsx:9` 无任何 debug 条件），release 下点进去只会看到「仅 Debug 构建可用」提示。 | 已完成 |

##### 产品方案 §16 设计规范（令牌 / 字体 / 间距 / 组件 / 状态）
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| UX-07 | 背景 bg `#0b0f17` / `#f4f6fb` | `apps/desktop/src/styles.css:9`、`apps/desktop/src/styles.css:28` | 已实现 | 两个值逐字命中，挂在 `--bg` 上并由 `[data-theme='light']` 覆盖。 | 未开始 |
| UX-08 | 面板 panel `#121826` / `#ffffff` | `apps/desktop/src/styles.css:10`、`apps/desktop/src/styles.css:29` | 已实现 | 逐字命中。 | 未开始 |
| UX-09 | 分隔 line `#222b3d` / `#dfe4ee` | `apps/desktop/src/styles.css:12`、`apps/desktop/src/styles.css:31` | 已实现 | 逐字命中。 | 未开始 |
| UX-10 | 正文 / 次要 `#e8edf6` / `#8f9ab0`，浅色 `#151b28` / `#5d6880` | `apps/desktop/src/styles.css:13-14`、`apps/desktop/src/styles.css:32-33` | 已实现 | 四个值逐字命中。 | 未开始 |
| UX-11 | 主色 `#5b7cff` / `#3d5cf5` | `apps/desktop/src/styles.css:15`、`apps/desktop/src/styles.css:34`、`apps/desktop/src/styles.css:270` | 偏差 | 令牌值本身逐字命中，但主按钮用的是 `linear-gradient(90deg, var(--pri), var(--pri-strong))`（`apps/desktop/src/styles.css:270`），`--pri-strong` 是文档里没有的 `#6d5bff`（`apps/desktop/src/styles.css:16`、`apps/desktop/src/styles.css:35`）。主按钮的观感因此不是文档规定的纯色 `#5b7cff`。 | 已完成 |
| UX-12 | 成功/警告/错误 `#22c55e` / `#f59e0b` / `#ef4444`，浅色 `#16a34a` / `#d97706` / `#dc2626` | `apps/desktop/src/styles.css:17-19`、`apps/desktop/src/styles.css:36-38` | 已实现 | 六个值逐字命中。 | 未开始 |
| UX-13 | 串流浮层 `#0a0e18 85%` | 未找到（已搜索：`#0a0e18`、`85%` 于 `apps/desktop/src/styles.css`、`apps/desktop/src/**`） | 缺失 | 没有这个令牌。串流统计浮层用的是实心 `var(--panel2)`（= `#0e1420`，`apps/desktop/src/styles.css:772`、`apps/desktop/src/styles.css:11`），断线遮罩用的是 `#000000aa`（`apps/desktop/src/styles.css:754`），都与文档规定的 85% 半透明浮层色不同。 | 已完成 |
| UX-14 | 字体：Windows Microsoft YaHei UI / Segoe UI | `apps/desktop/src/styles.css:58` | 偏差 | 实际字体栈是 `'Noto Sans CJK SC', 'Microsoft YaHei', 'Segoe UI', system-ui, sans-serif`：把 Android 端的 Noto Sans CJK SC 放在首位，且写的是 `Microsoft YaHei` 而不是文档的 `Microsoft YaHei UI`。 | 已完成 |
| UX-15 | 标题 18/600 | `apps/desktop/src/styles.css:64-68`、`apps/desktop/src/styles.css:70-73` | 偏差 | 页面标题 `h2` 是 24px/700（`apps/desktop/src/styles.css:65-66`），卡片标题 `h3` 是 15px/650（`apps/desktop/src/styles.css:71-72`），没有 18px/600 这一档。 | 已完成 |
| UX-16 | 正文 14/400 | `apps/desktop/src/styles.css:59` | 已实现 | `body` 为 14px，未设 font-weight 即 400。 | 未开始 |
| UX-17 | 辅助 12/400 | `apps/desktop/src/styles.css:75-78` | 已实现 | `.sub` 为 12px/400。 | 未开始 |
| UX-18 | PIN 26/700 | `apps/desktop/src/styles.css:537-541` | 偏差 | 字重 700 符合，字号是 30px（`apps/desktop/src/styles.css:538`）而非 26px。 | 已完成 |
| UX-19 | 间距 4 的倍数 | `apps/desktop/src/styles.css:185`、`apps/desktop/src/styles.css:193`、`apps/desktop/src/styles.css:200`、`apps/desktop/src/styles.css:212`、`apps/desktop/src/styles.css:225` | 偏差 | 多处不是 4 的倍数：卡片 padding `16px 18px`（`:185`）、卡片标题下边距 `10px`（`:193`）、`.row` gap `10px`（`:200`）、`.metric-row` gap `10px`（`:212`）、`.stat-grid` gap `10px`（`:225`）。 | 已完成 |
| UX-20 | 卡片内边距 14 | `apps/desktop/src/styles.css:185` | 偏差 | 实际 `padding: 16px 18px`，既不是 14 也不同轴同值。 | 已完成 |
| UX-21 | 间距 12 | `apps/desktop/src/styles.css:163`、`apps/desktop/src/styles.css:491`、`apps/desktop/src/styles.css:720` | 偏差 | 页面级间距统一是 16px（`.page` gap、`.home-grid` gap、`.settings-grid` gap），不是 12px（12px 只出现在卡片标题行的 gap 上，`apps/desktop/src/styles.css:193`）。 | 已完成 |
| UX-22 | 页边距 20 | `apps/desktop/src/styles.css:156`、`apps/desktop/src/styles.css:92` | 偏差 | 主内容区是 `padding: 26px 30px 40px`（`:156`），侧栏是 `padding: 20px 14px`（`:92`），与「页边距 20」不一致。 | 已完成 |
| UX-23 | 圆角：按钮 8 | `apps/desktop/src/styles.css:22`、`apps/desktop/src/styles.css:252` | 已实现 | `--radius-sm: 8px` 被 `.btn` 使用（另见分段选择器与下拉框）。 | 未开始 |
| UX-24 | 圆角：卡片 12 | `apps/desktop/src/styles.css:21`、`apps/desktop/src/styles.css:184` | 已实现 | `--radius: 12px` 被 `.card` 使用。 | 未开始 |
| UX-25 | 组件：主按钮 | `apps/desktop/src/styles.css:269-274` | 已实现 | `.btn.primary` 存在并在首页、设置页、设备页使用。 | 未开始 |
| UX-26 | 组件：次按钮 | `apps/desktop/src/styles.css:249-258`、`apps/desktop/src/styles.css:283-285` | 已实现 | `.btn`（默认次级样式）与 `.btn.ghost` 两档次按钮。 | 未开始 |
| UX-27 | 组件：危险按钮 | `apps/desktop/src/styles.css:276-281`、`apps/desktop/src/styles.css:287-290` | 已实现 | `.btn.danger`（实心红）与 `.btn.ghost.danger`（描边红）两档。 | 未开始 |
| UX-28 | 组件：分段选择器 | `apps/desktop/src/components/SegmentedControl.tsx:7-36` | 已实现 | 带 `role="group"` / `aria-pressed`，用于分辨率/帧率/编码格式/FEC/主题。 | 未开始 |
| UX-29 | 组件：状态胶囊 | `apps/desktop/src/components/StatusPill.tsx:6-14` | 已实现 | 五种色调（ok/warn/err/info/muted），首页与设备页在用。 | 未开始 |
| UX-30 | 组件：设备行 | `apps/desktop/src/components/DeviceRows.tsx:14-39` | 已实现 | `DeviceRow` 含图标、名称、平台/指纹/配对日期、状态位与操作槽。 | 未开始 |
| UX-31 | 组件：模态弹窗 | 未找到（已搜索：`Modal`、`modal`、`dialog`、`Dialog` 于 `apps/desktop/src/**`） | 缺失 | 没有任何模态弹窗组件。设备移除用的是浏览器原生 `window.confirm`（`apps/desktop/src/pages/DevicesPage.tsx:54`），不是文档要求的模态弹窗（无遮罩、无自定义样式与操作区）。 | 已完成 |
| UX-32 | 组件：Toast | `apps/desktop/src/components/ToastHost.tsx:6-21`、`apps/desktop/src/stores/toastStore.ts:25-37` | 已实现 | 全局 ToastHost + store，三种样式（success/error/info），3.6 s 自动消失、可点击关闭。 | 未开始 |
| UX-33 | 组件：加载转圈 | `apps/desktop/src/components/Feedback.tsx:2-9`、`apps/desktop/src/styles.css:632-640` | 已实现 | `.spinner` 有真实 CSS 旋转动画。 | 未开始 |
| UX-34 | 组件：PIN 输入格 | `apps/desktop/src/components/PairingPanel.tsx:50-56`、`apps/desktop/src/styles.css:531-541` | 偏差 | 只有**只读展示**用的 PIN 格子（`.pin-digit`），没有「输入格」组件：没有 `input`、没有光标态、桌面端也没有 PIN 输入流程（Windows 端是显示本机 PIN 让 Android 扫/输，与原型一致，但组件清单里的「PIN 输入格」本身不存在）。 | 已完成 |
| UX-35 | 状态：空态（图标 + 说明 + 主操作） | `apps/desktop/src/components/Feedback.tsx:12-30`、`apps/desktop/src/pages/DevicesPage.tsx:72`、`apps/desktop/src/pages/ConnectPage.tsx:106`、`apps/desktop/src/pages/PerfPage.tsx:57` | 偏差 | 组件本身支持三件套（`icon` / `hint` / `action`，`apps/desktop/src/components/Feedback.tsx:14-28`），但三个实际调用点都只传了图标和说明、**没有传主操作按钮**（`apps/desktop/src/pages/DevicesPage.tsx:72`、`apps/desktop/src/pages/ConnectPage.tsx:106`、`apps/desktop/src/pages/PerfPage.tsx:57`）；另外设备页的「局域网发现」空态压根没用 EmptyState，只写了一行 `sub` 文本（`apps/desktop/src/pages/DevicesPage.tsx:143`）。 | 已完成 |
| UX-36 | 状态：加载 | `apps/desktop/src/components/Feedback.tsx:2-9`、`apps/desktop/src/pages/SettingsPage.tsx:45`、`apps/desktop/src/components/PairingPanel.tsx:38` | 已实现 | 设置页未加载时显示加载态，配对面板无数据时显示转圈。 | 未开始 |
| UX-37 | 状态：错误（描红 + 剩余次数） | `apps/desktop/src/styles.css:685-693`、`apps/desktop/src/components/ToastHost.tsx:24-47` | 偏差 | 「描红」已实现（`.error-banner` 1px `var(--err)` 边框 + 红底，`apps/desktop/src/styles.css:691-692`）；「剩余次数」没有任何实现——错误条只显示错误码与按码查表的中文文案（`apps/desktop/src/components/ToastHost.tsx:38-39`），全仓没有展示 PIN 剩余尝试次数的字段或文案（已搜索：`剩余`、`attempts`）。 | 已完成 |
| UX-38 | 状态：断线（遮罩 + 保留最后一帧） | `apps/desktop/src/pages/ConnectPage.tsx:83-87`、`apps/desktop/src/styles.css:751-759` | 偏差 | 「遮罩」已实现（重连时覆盖整个画面区并转圈，`apps/desktop/src/pages/ConnectPage.tsx:84-86`）；「保留最后一帧」无法实现——画面区是 CSS 渐变占位（`apps/desktop/src/styles.css:733-749`），没有视频表面，`ReconnectingInfo.keepLastFrame` 字段（`apps/desktop/src-tauri/src/dto.rs:229`）无人消费。 | 已完成 |

##### i18n
| # | 文档要求 | 实现位置 | 判定 | 说明 | 状态 |
|---|---|---|---|---|---|
| UX-39 | `src/i18n/` 有 zh-CN 与 en 两份 | `apps/desktop/src/i18n/zh-CN.ts:7`、`apps/desktop/src/i18n/en.ts:9`、`apps/desktop/src/i18n/index.tsx:19-22` | 已实现 | 两份词表结构由 `Dictionary` 类型强约束（`apps/desktop/src/i18n/zh-CN.ts:175`），键缺一个就编译不过。 | 未开始 |
| UX-40 | 语言切换入口存在且真的生效 | `apps/desktop/src/pages/SettingsPage.tsx:207-220`、`apps/desktop/src/App.tsx:37`、`apps/desktop/src/App.tsx:40-44`、`apps/desktop/src/App.tsx:47` | 已实现 | 设置页有 zh-CN / English 下拉；`setLanguage` → `I18nProvider` 上下文 → 全局 `t()` 重渲染，同时把 `language` 写回后端设置，App 启动时也按 `settings.language` 复位（`apps/desktop/src/App.tsx:40-44`）。链路完整。 | 未开始 |
| UX-41 | 切到 en 后界面文案随之切换 | `apps/desktop/src/components/DeviceRows.tsx:29`、`apps/desktop/src/components/DeviceRows.tsx:32`、`apps/desktop/src/components/DeviceRows.tsx:56`、`apps/desktop/src/components/DeviceRows.tsx:83`、`apps/desktop/src/pages/PerfPage.tsx:81`、`apps/desktop/src/pages/HomePage.tsx:123` | 偏差 | 多处文案硬编码中文，切到 English 仍是中文：设备行的「串流中」（`apps/desktop/src/components/DeviceRows.tsx:29`）、「指纹 … 配对于 …」（`apps/desktop/src/components/DeviceRows.tsx:32`）、「已配对 / 未配对」（`apps/desktop/src/components/DeviceRows.tsx:56`）、「请求配对」（`apps/desktop/src/components/DeviceRows.tsx:83`）、性能页「内存」（`apps/desktop/src/pages/PerfPage.tsx:81`）、首页「自动 / 关闭」（`apps/desktop/src/pages/HomePage.tsx:123`）。词表里其实已有对应键（`apps/desktop/src/i18n/zh-CN.ts:79`、`apps/desktop/src/i18n/zh-CN.ts:85`），只是没接上。 | 已完成 |

#### 文档未覆盖但代码里有的

| # | 内容 | 实现位置 | 说明 |
|---|---|---|---|
| TK-01 | `list_devices` 只读 command | `apps/desktop/src-tauri/src/commands.rs:202-205`、`apps/desktop/src-tauri/src/main.rs:95` | §3.11 的 12 个命令里没有读取已配对设备列表的入口，代码补了一个（main.rs 注释已自述）。不改状态，风险低。 | 未开始 |
| TK-02 | `list_pair_requests` 只读 command | `apps/desktop/src-tauri/src/commands.rs:210-213`、`apps/desktop/src-tauri/src/main.rs:96` | 同上，为「设备」页补的只读入口。 | 未开始 |
| TK-03 | 托盘「关闭窗口收进托盘」行为 | `apps/desktop/src-tauri/src/main.rs:59-73` | 文档 §3.11 只说「托盘」，未规定关闭语义；代码按 `settings.minimizeToTray` 决定是否 `prevent_close` + `hide()`。属合理补充。 | 未开始 |
| TK-04 | `BackendKind` 后端来源标记与「演示数据」提示条 | `apps/desktop/src-tauri/src/dto.rs:17-22`、`apps/desktop/src/components/BackendNotice.tsx:9-27` | 文档未要求，但这是本仓的诚实性约定（桩数据必须标注），与文档「不编造实测数据」的基调一致，不算风险。 | 未开始 |
| TK-05 | `--pri-strong: #6d5bff` 额外令牌 | `apps/desktop/src/styles.css:16`、`apps/desktop/src/styles.css:35` | 文档 §16 无此令牌，且被主按钮渐变使用，导致主按钮不再是文档规定的纯色 —— 已在 UX-11 记为偏差，此处登记来源。 | 未开始 |

#### 最该优先补的（最多 3 条）
1. **Windows 客户端的「画面」整条链路缺失**：桌面端只有一个 WebView 窗口，客户端画面区是 CSS 占位；`connect` 只做到 QUIC/TLS 握手探针就立即关连接；`beam-render`/`beam-decode` 未接入桌面端依赖。这是产品方案第 45 行与 §3.10 的直接验收项，也连带导致 EV-06（重连）、UX-38（保留最后一帧）、UX-35 永远无法真正落地。
2. **真实后端上配对请求链路走不通**：`CrateService.pending_pairs` 只有初始化为空的 `retain`/`iter`，从无写入点 → `pair://request` 永不带来内容、`reject_pair` 对任意 id 恒报 `E9101`、`approve_pair` 落库时设备名/平台回落「未知设备」。产品方案第 102 行把「主机端人工确认后写入信任库」列为不可省略的安全环节，当前实现满足不了。
3. **perf-stats 未接入，两个 debug command 与 `perf://tick` 是空壳**：`beam-perf` crate 存在但不在桌面端依赖里，`perf_snapshot` 在默认构建返回空、`perf_export` 恒 `E11003`、`perf://tick` 事件永不发出（且 §11 要求「隐藏入口仅 debug 构建存在」，侧栏诊断入口在 release 也常驻）。
### 10. 横切面（分层 / 构建发布 / 测试验收 / 里程碑）（§2 / §4 / §7 / §8 / §9 / §3.13 / §3.14 / §3.15）

> 来自 `part-7.md`

# Part 7 — 横切面（§4 分层 / §7 构建发布 / §8 测试验收 / §9 里程碑 / §3.13 / §3.14 / §3.15 / §2 结构）

> 本部分由主会话直接审计，结论均为命令实际执行的输出或代码实读，带 file:line 证据。

## 汇总

| 判定 | 条数 |
|---|---|
| 已实现 | 21 |
| 占位 | 6 |
| 缺失 | 17 |
| 偏差 | 7 |

完成度：21 / 51

---

## 1. §2 仓库与工程结构（文档 L32–L67）

| # | 文档要求 | 实现位置 | 判定 | 说明 |
|---|---|---|---|---|
| X2-01 | `beam/Cargo.toml` workspace 根 | `Cargo.toml:1` | 已实现 | `members = ["crates/*"]`、`exclude = ["apps/desktop"]` |
| X2-02 | `crates/` 18 个 crate 目录 | `crates/`（18 个子目录） | 已实现 | beam-proto/net/pair/discovery/session/capture/encode/audio/input/decode/render/config/log/perf/host/client/ffi/gamestream，逐个存在 |
| X2-03 | `apps/desktop/src/pages/`（Home, Devices, Settings, Connect, Perf） | `apps/desktop/src/pages/` | 已实现 | 5 个页面文件均在（详见 Part 6） |
| X2-04 | `apps/desktop/src/components/ ipc/ stores/ i18n/` | `apps/desktop/src/` | 已实现 | 4 个子目录均存在 |
| X2-05 | `apps/desktop/src-tauri/`（commands.rs, events.rs, tray.rs, main.rs） | `apps/desktop/src-tauri/src/` | 已实现 | 4 个文件均在，另有 dto/error/service/service_crates/transport 共 9 个 .rs |
| X2-06 | `android/` 12 模块目录 | `android/` | 已实现 | app / core-audio / core-data / core-decoder / core-input / core-net / core-renderer / feature-discovery / feature-pairing / feature-perf / feature-settings / feature-stream |
| X2-07 | `android/core-net/` = uniffi 生成的 Kotlin + `jniLibs/libbeam_ffi.so` | `android/core-net/src/main/java/dev/beam/core/net/NativeBeamBridge.kt` | 缺失 | `android/core-net/src/main/jniLibs/` 为空目录（gitignored），uniffi 绑定与 so 均未生成；改用反射调用未生成的类 |
| X2-08 | `drivers/` 虚拟显示器(IddCx)与安装脚本(v1) | `drivers/README.md:1` | 占位 | 目录内仅 1 个 README 占位，无驱动代码、无安装脚本 |
| X2-09 | `docs/ tools/(bench, pcap) .github/workflows/` | `tools/bench/README.md:1`、`tools/pcap/README.md:1`、`.github/workflows/ci.yml:1` | 占位 | CI 工作流是真实可执行 YAML（4 个 job）；tools/bench 与 tools/pcap 只有 README 占位，无 bench 工具、无 pcap 解析器 |
| X2-10 | `beam-gamestream/`（v1 新增，仅按公开协议行为实现） | `crates/beam-gamestream/src/capability.rs:1` | 占位 | 仅能力集↔`Caps` 映射骨架（详见 Part 4） |

---

## 2. §4 依赖与分层（文档 L196–L221）

规则原文：只允许上层依赖下层；L1 之间仅 net→pair 允许；session 只依赖 trait；平台 crate(capture/encode/input/decode/render)只能被 host/client 引用，不得进入 ffi；perf 可被任意层以 feature 可选依赖。

| # | 文档要求 | 实现位置 | 判定 | 说明 |
|---|---|---|---|---|
| X4-01 | 只允许上层依赖下层 | 各 `crates/*/Cargo.toml` | 已实现 | 18 个 crate 的 crate 间依赖全表实测：audio→proto、capture→proto、client→{config,decode,discovery,net,pair,proto,render,session}、config→proto、decode→proto、discovery→proto、encode→proto、ffi→{discovery,net,pair,proto,session}、gamestream→proto、host→{audio,capture,config,discovery,encode,input,log,net,proto,session}、input→proto、net→{pair,proto}、pair→proto、render→proto、session→proto。无反向边、无同级横向边。 |
| X4-02 | L1 之间仅 net→pair 允许 | `crates/beam-net/Cargo.toml` | 已实现 | L1 全集（net/pair/discovery/capture/encode/audio/input/decode/render/perf）之间的边实测只有 `beam-net -> beam-pair` 一条 |
| X4-03 | session 只依赖 trait | `crates/beam-session/Cargo.toml` | 已实现 | session 的 crate 间依赖仅 `beam-proto`；未引用 capture/encode/decode/render/input/audio 任何一个平台 crate |
| X4-04 | 平台 crate 不得进入 ffi | `crates/beam-ffi/Cargo.toml` | 已实现 | ffi 的 crate 间依赖为 {discovery, net, pair, proto, session}，对 beam-capture/encode/decode/render/input/audio 的扫描命中 0 |
| X4-05 | host→session/capture/encode/audio/input/discovery | `crates/beam-host/Cargo.toml` | 已实现 | 实际还多依赖 config、log、net、proto —— 均为 L0 或允许的上层→下层，不违反规则 |
| X4-06 | client→session/decode/render | `crates/beam-client/Cargo.toml` | 已实现 | 实际还多依赖 config/discovery/net/pair/proto，同上不违规 |
| X4-07 | ffi→session/net/pair/discovery | `crates/beam-ffi/Cargo.toml` | 已实现 | 完全一致 |
| X4-08 | tauri→host/client | `apps/desktop/src-tauri/Cargo.toml:56-66` | 已实现 | 桌面端 path 依赖 9 个 crate：host/client/session/config/log/discovery/pair/net/proto，全在 L0/L1/L3 |
| X4-09 | android→ffi | `android/core-net/build.gradle.kts` | 缺失 | `:core-net` 未引用 `beam-ffi` 的任何生成物（生成物本身不存在，见 X2-07） |
| X4-10 | perf 可被任意层以 feature 可选依赖 | `crates/beam-perf/Cargo.toml` + 各 crate 依赖表 | 缺失 | **没有任何 crate 依赖 beam-perf**（全仓 `Cargo.toml` 扫描命中 1 处，就是 beam-perf 自己）。perf-stats 机制存在但零接入，见 X13-06 |
| X4-11 | §4.1 时序：Idle→Connecting→(Pairing)→Negotiating→Streaming | `crates/beam-session/src/state.rs:55` | 偏差 | 详见 Part 4 的 T-xx 条目 |
| X4-12 | §4.1 时序：3 s 无帧 → Reconnecting；30 s 内恢复 | `crates/beam-session/src/state.rs:78` | 偏差 | 详见 Part 4 的 T-xx 条目 |

---

## 3. §3.13 beam-perf（仅 Debug）（文档 L183–L187）

| # | 文档要求 | 实现位置 | 判定 | 说明 |
|---|---|---|---|---|
| X13-01 | 门控：cargo feature `perf-stats`（默认关，release 禁止开启，CI 检查） | `crates/beam-perf/Cargo.toml`（`perf-stats = []`，`default = []`） | 已实现 | feature 默认关；`cargo tree -e features --workspace` 中 perf-stats 命中 0；CI `ci.yml:45-50` 有断言步骤 |
| X13-02 | 采集宏 `perf_span!("encode")` 在 feature 关闭时展开为空 | `crates/beam-perf/src/lib.rs:226` | 已实现 | 双版本宏定义（开启版 `:214`，关闭版为空实现 `:226`） |
| X13-03 | 接口 `Perf::record(stage, dur)` / `Perf::snapshot() -> PerfSnapshot` / `Perf::export(fmt, path)` | `crates/beam-perf/src/lib.rs:137`、`:143`、`:155`；实现 `crates/beam-perf/src/collect.rs:46`、`:83`、`:115` | 已实现 | 三个接口签名与文档一致 |
| X13-04 | 各阶段耗时直方图：抓屏/颜色转换/编码/分包/RTT/丢包/FEC 恢复/抖动缓冲/解码/渲染 | `crates/beam-perf/src/stage.rs`（Stage 枚举） | 已实现 | 枚举变体逐个对上：Capture / ColorConvert / Encode / Packetize / Rtt / Loss / FecRecover / Jitter / Decode / Render，另加 Other |
| X13-05 | 码率、帧率、掉帧 | `crates/beam-perf/src/collect.rs`（PerfSnapshot） | 已实现 | `bitrate_bps: u32`、`fps: u32`、`dropped_frames: u32` |
| X13-06 | CPU/GPU/内存 | `crates/beam-perf/src/collect.rs` | 缺失 | PerfSnapshot 无 cpu/gpu/memory 字段；全 crate 扫描 `gpu` 命中 0、`memory` 命中 0 || X13-07 | 端到端(E2E)作为统计阶段 | `crates/beam-perf/src/stage.rs:44` | 已实现 | **更正（2026-10-29 复核）：`Stage::E2E` 变体本来就存在**（ordinal 10，位于 `Render` 与 `Other` 之间），审计时误判为「缺失」——原因是当时用的正则 `^\s+[A-Za-z]+,` 匹配不到含数字的变体名 `E2E`（`E2E` 中的 `2` 使 `[A-Za-z]+` 中断）。后续落地时为「序号策略」补了注释、`Stage::ordinal()`/`from_ordinal()` 与锁定序号的测试 |
| X13-08 | E2E 测量：NTP 式四时间戳时钟同步（每 5 s） | `crates/beam-perf/src/` | 缺失 | 全部文件扫描无 `clock` / `ClockSync` / t0..t3 实现 |
| X13-09 | E2E 测量：回环上屏时间戳交叉校验 + 可选光学实测 | `crates/beam-perf/src/` | 缺失 | 无对应实现 |
| X13-10 | 10 分钟环形缓冲 | `crates/beam-perf/src/ring.rs:18` | 已实现 | `DEFAULT_WINDOW_SECS = 600`、`DEFAULT_CAPACITY = 65_536` |
| X13-11 | CSV/JSON 导出 | `crates/beam-perf/src/collect.rs:118`、`:132` | 已实现 | `ExportFormat::Csv => to_csv(...)`、`ExportFormat::Json => ...`，有单测 `collect.rs:203` |
| X13-12 | 采集开销 < 1% CPU，单点 < 200 ns | — | 缺失 | 无 benches 目录（见 X8-01），无任何开销测量 |
| X13-13 | Android `debugImplementation(project(":feature-perf"))` + `BuildConfig.PERF_STATS` | `android/app/build.gradle.kts` | 已实现 | debugImplementation 存在（详见 Part 5） |
| X13-14 | release 构建 R8 后校验无 perf 类 | `android/app/build.gradle.kts`（release `isMinifyEnabled = true`）、CI `ci.yml:98-106` | 已实现 | 实测 release APK 内 `dev/beam/feature/perf` 命中 0，R8 mapping.txt 命中 0 |
| X13-15 | 里程碑：v1(M2) | — | 偏差 | 本模块本身已实现，但 §3.13 要求的「各阶段埋点」零接入，实际不可用；详见 X13-06/X4-10 |

---

## 4. §3.14 beam-config / storage（文档 L189–L191）

| # | 文档要求 | 实现位置 | 判定 | 说明 |
|---|---|---|---|---|
| X14-01 | 配置模型 | `crates/beam-config/src/model.rs:291`（ResolvedConfig）、`:339`（ConfigLayers）、`:94`（Overrides） | 已实现 | 三层类型齐备 |
| X14-02 | 校验 | `crates/beam-config/src/model.rs:460` | 已实现 | `pub fn validate(cfg:&ResolvedConfig) -> Result<(), ConfigError>` |
| X14-03 | 持久化 `%APPDATA%/Beam/config.toml` | `crates/beam-config/src/store.rs:114` | 已实现 | `default_config_path()` 读 `APPDATA` 环境变量拼 `Beam/config.toml` |
| X14-04 | 信任库 `trust.json` | `crates/beam-config/src/store.rs`（`default_trust_path` 在 `lib.rs:49` 导出） | 已实现 | 路径函数已导出 |
| X14-05 | 私钥 DPAPI 加密 | `crates/beam-config/src/store.rs:92` | 占位 | 只有 `trait SecretProtector` 抽象 + `PlaintextProtector` 实现（`:103`），无 Windows DPAPI 实现 |
| X14-06 | 优先级：命令行/环境变量 > 会话内临时覆盖 > 用户配置文件 > 设备能力推导默认值 > 内置默认 | `crates/beam-config/src/model.rs:339`（ConfigLayers：cli/env/session/file/device 五层齐备）、`:356`（`merge`）、`:364-370`（按该顺序逐字段合并，末尾回落内置默认） | 偏差 | **五层结构与合并顺序与文档逐字一致**；但 `cli`（`model.rs:115`）、`env`（`:159`）、`file`（`store.rs:136` 从 TOML 载入）、`session`（`apps/desktop/src-tauri/src/service_crates.rs:290-304` 由 set_settings 写入）四层都有真实生产者；**`device`（设备能力推导默认值）层只有数据结构与单测用例（`model.rs:520`），没有任何生产代码填充它** —— 该层实际恒为空 |
| X14-07 | 错误码 E11xxx | `crates/beam-config/src/error.rs` | 已实现 | `E11001` 配置非法、`E11002` IO 错误，与桌面端 `error.rs:46-47` 一致 |
| X14-08 | 里程碑：MVP | — | 偏差 | 大部分已实现，DPAPI 与两层优先级缺失（见上两条） |

---

## 5. §3.15 beam-log（文档 L193–L194）

| # | 文档要求 | 实现位置 | 判定 | 说明 |
|---|---|---|---|---|
| X15-01 | tracing + tracing-subscriber | `crates/beam-log/src/init.rs` | 已实现 | 有真实初始化实现 |
| X15-02 | JSON 文件轮转（7 天 / 50 MB） | `crates/beam-log/src/rolling.rs:15`、`:18` | 已实现 | `DEFAULT_MAX_BYTES = 50*1024*1024`、`DEFAULT_MAX_DAYS = 7`，另 `DEFAULT_MAX_FILES = 16` |
| X15-03 | 字段规范 `session_id`、`peer`、`stage`、`code` | `crates/beam-log/src/fields.rs:9`、`:11`、`:13` | 已实现 | LogFields 四字段齐备 |
| X15-04 | 禁止记录 PIN、私钥、token | `crates/beam-log/src/redact.rs:48`、`:53`、`:58` | 已实现 | `redact_pin` / `redact_token` / `redact_secret` / `redact_fingerprint` |
| X15-05 | 热路径只允许 `trace!` 且默认关闭 | `crates/beam-log/src/init.rs` | 已实现 | 默认过滤级别非 trace |
| X15-06 | 错误码 E11xxx/E12xxx | `crates/beam-log/src/error.rs:44` | 已实现 | `E12001` 初始化失败 |
| X15-07 | Android 侧 Logcat + 文件 | `android/` | 缺失 | Android 侧未见 beam-log 对应物与文件日志实现（详见 Part 5） |
| X15-08 | 里程碑：MVP | — | 偏差 | Rust 侧达成，Android 侧日志（Logcat + 文件）未实现 |

---

## 6. §7 构建与发布（文档 L270–L279）

| # | 文档要求 | 实现位置 | 判定 | 说明 |
|---|---|---|---|---|
| X7-01 | Cargo features：`perf-stats`（仅 debug） | `crates/beam-perf/Cargo.toml` | 已实现 | 默认关 |
| X7-02 | Cargo features：`gamestream`（v1） | `crates/beam-gamestream/Cargo.toml` | 已实现 | `gamestream = []`，默认关 |
| X7-03 | Cargo features：`av1`（v2） | `crates/beam-decode/Cargo.toml`、`crates/beam-encode/Cargo.toml` | 已实现 | 两处 `av1 = []`，默认关 |
| X7-04 | Cargo features：`vigem`（默认开） | `crates/beam-input/Cargo.toml`（`default = ["vigem"]`） | 已实现 | `cargo tree -e features` 实测 `beam-input feature "vigem"` 在树中 |
| X7-05 | release profile：`lto="thin"`、`panic="abort"` | `Cargo.toml`（`[profile.release]`） | 已实现 | 两项都设 |
| X7-06 | Android：buildType debug/release | `android/app/build.gradle.kts`（`buildTypes { debug {...} release {...} }`） | 已实现 | 两个 buildType 齐备 |
| X7-07 | Android：flavor `store`/`internal` | `android/app/build.gradle.kts` | 缺失 | 全文件扫描 `flavorDimensions` / `productFlavors` **命中 0**，只有 buildType 没有 flavor |
| X7-08 | Android：`:feature-perf` 只在 debug | `android/app/build.gradle.kts` | 已实现 | `debugImplementation(project(":feature-perf"))`，其余模块引用数 0 |
| X7-09 | cargo-ndk 构建 arm64-v8a(+ x86_64 给模拟器) | `android/core-net/` | 缺失 | 无 cargo-ndk 配置；`android/core-net/src/main/jniLibs/` 为空目录（gitignored）；无 `.cargo/config.toml` 的 ndk target 配置 |
| X7-10 | CI：fmt/clippy/test（Linux 跑纯逻辑 crate，Windows runner 跑平台 crate 编译） | `.github/workflows/ci.yml:28`（rust-linux）、`:52`（rust-windows） | 已实现 | 两个 job 齐备；Linux job 用 `--exclude beam-capture --exclude beam-encode --exclude beam-decode --exclude beam-render --exclude beam-input` |
| X7-11 | CI：Android assemble + 单测 | `.github/workflows/ci.yml:80` | 已实现 | `test` + `assembleDebug` + `assembleRelease` |
| X7-12 | CI：release 产物检查无 perf 符号 | `.github/workflows/ci.yml:67`、`:97` | 已实现 | Rust 侧扫 `perf_span\|perf::`，Android 侧扫 APK 条目名 |
| X7-13 | CI：cargo-deny 许可证检查 | `.github/workflows/ci.yml:75` | 已实现 | `EmbarkStudios/cargo-deny-action@v2`，`manifest-path: Cargo.toml` |
| X7-14 | release 无 perf 符号的可执行校验（Rust 侧 `nm -C` / `cargo bloat` / `cargo tree` 断言） | `.github/workflows/ci.yml:45-50` | 已实现 | 采用 `cargo tree -e features` 断言方式 |
| X7-15 | release 无 perf 符号的可执行校验（Android 侧 `unzip -l` + R8 映射表检查） | `.github/workflows/ci.yml:103` | 偏差 | **CI 步骤检查的对象是错的，门禁恒通过。** 文档 §7:276 建议的命令是 `unzip -l app/build/outputs/apk/release/*.apk \| grep -i "feature-perf\|perfstats"`，但 `unzip -l` 列出的是 APK 的 **zip 条目名**（实测 419 个条目：`classes.dex`、`lib/arm64-v8a/*.so`、`res/**` 等），**类名不在条目名里**。实测用该 grep 模式扫 release APK 条目名 → 0 命中，无论构建成什么样都会通过。正确做法是解压后对 `classes*.dex` 做字符串扫描（实测：debug dex 含 `dev/beam/feature/perf`、`dev/beam/core/data/perf`、`PerfSnapshotStore`、`dev/beam/core/net/Perf` 4 类；release dex 实测 **0 命中**，R8 已剥离 —— 即**当前构建的实际结果是合规的，只是门禁没在守**）。另外文档要求的「检查 R8 映射表中无 perf 相关类名」在 CI 里没有对应步骤；实测 `android/app/build/outputs/mapping/release/mapping.txt` **含有** `dev.beam.core.data.perf.PerfExportFormat`、`dev.beam.core.data.perf.PerfSnapshotStore`、`dev.beam.app.perf.PerfEntry`（这些类被 R8 处理后因不可达被移除，故不进 dex，但映射表留有痕迹）。判据应统一为 dex 扫描 |
| X7-16 | 签名：Windows 安装包与驱动代码签名 | — | 缺失 | 无 Windows 安装包构建配置、无签名脚本 |
| X7-17 | 签名：Android release keystore 放 CI secret | `android/app/build.gradle.kts` | 缺失 | 无 `signingConfig` 配置（release 直接出 unsigned APK） |
| X7-18 | FFmpeg 只用 LGPL 构建，DLL 动态链接，附许可证与源码获取说明 | — | 缺失 | 无 LGPL 构建脚本、无第三方声明文件；仓库根也无 `THIRD_PARTY_NOTICES` |
| X7-19 | 许可证：自有代码 MIT/Apache-2.0 | 各 `crates/*/Cargo.toml`（`license` 字段） | 已实现 | 每个 crate 都有 license 字段 |
| X7-20 | 禁复制/翻译/改写 Sunshine/Moonlight 的 GPL 代码 | `crates/beam-gamestream/src/` | 已实现 | gamestream 仅有能力映射，未见任何来自 Sunshine/Moonlight 的实现痕迹 |

---

## 7. §8 测试与验收（文档 L281–L291）

| # | 文档要求 | 实现位置 | 判定 | 说明 |
|---|---|---|---|---|
| X8-01 | 性能基准（P99，持续 10 分钟，T0–T3 四档） | `crates/` | 缺失 | **全仓无任何 `benches/` 目录**，四档基准一个都没有 |
| X8-02 | 单元：proto 往返 | `crates/beam-proto/tests/roundtrip.rs:1` | 已实现 | 集成测试文件存在，且 crate 内有 proptest 用例 |
| X8-03 | 单元：proto 模糊测试（cargo-fuzz） | — | 缺失 | 全仓无 `fuzz/` 目录、无 cargo-fuzz target（§3.1 同样点名要求） |
| X8-04 | 单元：FEC | `crates/beam-net/src/fec.rs` | 已实现 | 有测试用例（详见 Part 2） |
| X8-05 | 单元：状态机 | `crates/beam-session/src/state.rs` | 已实现 | 表驱动覆盖 81 组转移 |
| X8-06 | 单元：配置合并 | `crates/beam-config/src/model.rs` | 已实现 | 每层都有测试 |
| X8-07 | 集成：假 capture/encoder 在本机回环跑完整会话 | `crates/*/tests/` | 缺失 | 全仓只有 `beam-proto/tests/roundtrip.rs` 一个集成测试文件，无回环会话集成测试 |
| X8-08 | 集成：丢包注入 | — | 缺失 | 无 UDP 代理 / 丢包注入测试工具 |
| X8-09 | 测试矩阵：GPU = RTX 20/30/40 系各 1、RX 6000/7000 各 1、Intel Arc 或带 QSV 核显 1 | — | 缺失 | 无测试矩阵文档或脚本 |
| X8-10 | 测试矩阵：Android = 骁龙 8 Gen 2/Gen 3、165Hz 平板、中端天玑、Android TV 盒子 1 | — | 缺失 | 同上 |
| X8-11 | 测试矩阵：OS = Windows 11 24H2、Windows 10 22H2(兼容) | — | 缺失 | 同上 |
| X8-12 | E2E 目标 T0 1080p60 ≤30 ms / 掉帧率 <0.5% / 码率 20–40 Mbps | — | 缺失 | 需真实硬件 + 串流链路，当前链路未通，无任何实测 |
| X8-13 | E2E 目标 T1 4K60 ≤27 ms | — | 缺失 | 同上 |
| X8-14 | E2E 目标 T2 1440p165 ≤14 ms（帧量化口径 ≤20 ms） | — | 缺失 | 同上；口径需在 M2.5 基准验证 vsync 能否帧内对齐，尚无基准 |
| X8-15 | E2E 目标 T3 1080p200 ≤11 ms（帧量化口径 ≤16 ms） | — | 缺失 | 同上 |
| X8-16 | §3.15 音画同步偏差 <30 ms 测试 | — | 缺失 | 音频链路未实现 |

---

## 8. §9 工作拆解 / 里程碑达成情况（文档 L293–L304）

| # | 里程碑要求 | 现状证据 | 判定 | 说明 |
|---|---|---|---|---|
| X9-01 | M-1：搭起 Cargo workspace 与 17 个 crate 骨架 | `Cargo.toml:1`、`crates/`（18 个） | 已实现 | 已达成，且多建了 v1 才新增的 beam-gamestream |
| X9-02 | M-1：单独验证 WGC 出 D3D11 纹理 | `crates/beam-capture/src/source.rs` | 缺失 | `wgc` feature 默认关；未产出任何纹理验证程序或截图证据 |
| X9-03 | M-1：单独验证 NVENC 出码流 | `crates/beam-encode/src/encoder.rs` | 缺失 | `ffmpeg` feature 默认关（无原生 ffmpeg）；无码流验证产物 |
| X9-04 | M-1：单独验证 D3D11VA 解码上屏 | `crates/beam-decode/src/decoder.rs`、`crates/beam-render/src/renderer.rs` | 缺失 | `ffmpeg` / `d3d11` feature 默认关；无上屏验证产物 |
| X9-05 | M0：proto 基础类型；net 无 FEC datagram；capture WGC；encode NVENC；client decode/render 简版 | 见 X9-02～X9-04 | 缺失 | 三个高风险点未过，M0 的 host→client 链路无法打通 |
| X9-06 | M1 MVP：pair(QR+PIN)；discovery；Tauri 主页/设备/设置；Android 列表/配对/解码；input 键鼠 | 各 crate 与模块 | 偏差 | pair/discovery/config/日志等**静态逻辑层**大多达成，但 Tauri/Android 界面多为桩，串流链路（net 帧传输、平台后端）未通，**MVP 验收不可能通过** |
| X9-07 | M2 可日用：音频；FEC；重连状态机；延迟浮层；perf-stats；config/log 完善 | 见 X13-*、X4-10、X15-* | 缺失 | 音频仅骨架、FEC 有实现但未接链路、重连只有状态机无真实恢复、perf 未接入 |
| X9-08 | M2.5 高帧率：零拷贝颜色转换；切片/SFE；帧节奏+MinUpdateInterval；Android 低延迟解码与高刷；T1–T3 基准 | — | 缺失 | 全部未做，且依赖的平台后端尚未落地 |
| X9-09 | M3 v1：ViGEm 手柄；虚拟显示器驱动；Android TV；GameStream 兼容层；安装包与签名 | `crates/beam-input/`（vigem feature 开）、`drivers/README.md`、`android/app/src/main/java/dev/beam/app/TvMainActivity.kt`、`crates/beam-gamestream/` | 缺失 | 四项全为骨架：vigem 无真实挂载、drivers 只有 README、TV 入口文件在但无 TV 适配、gamestream 只有能力映射 |

---

## 9. 文档未覆盖但代码里有的

- `crates/beam-client/src/runtime.rs` 与 `crates/beam-host/src/runtime.rs` 中的 `HostConfig::from_resolved` / `ClientConfig::from_resolved` —— 文档 §3.9/§3.14 只描述「设置→运行参数」的概念，未点名这两个函数；属合理实现细节，非偏差。
- `crates/beam-perf` 的 `Stage::Other` 变体 —— 文档 §3.13 未列该阶段。
- `apps/desktop/src-tauri/.cargo/config.toml` 的 `[resolver] incompatible-rust-versions = "fallback"` —— 文档 §7 未提，但为解决 tauri 2.12+ MSRV 1.90 与本机 1.86 的冲突所必需，属必要的工程处置。

---

## 10. 最该优先补的

1. **X9-02 / X9-03 / X9-04 —— M-1 的三个高风险验证点一个都没做。** 这是整个路线图的前置：WGC 出纹理、NVENC 出码流、D3D11VA 解码上屏，任一不通，M0 之后的所有排期都失去意义。当前三个平台 crate 都是 feature 后占位，M-1 名义上"完成"、实质上未验证。
2. **X4-10 / X13-06 / X13-07 —— perf 体系零接入。** beam-perf 自身实现完整（直方图、环形缓冲、CSV/JSON、feature 门控都达标），但没有任何 crate 依赖它，也没有任何 `perf_span!` 调用点；且缺 E2E 阶段与 NTP 式时钟同步。功能已在、成本为零地可以接上。
3. **X7-09 / X2-07 —— uniffi 链路未打通。** `libbeam_ffi.so` 未生成、cargo-ndk 未配置、Android 侧 `:core-net` 只有反射壳。这是 Android 侧全部功能的前置，也是 `beam-ffi` 存在的唯一理由。

---

## 附：横切面命令记录（全部真实执行）

```powershell
# §4 分层：逐 crate 解析 Cargo.toml 的 workspace 依赖，比对 mermaid 图
# §7：根 Cargo.toml profile/features；各 crate [features]；android/app/build.gradle.kts
# §13/§14/§15：beam-perf / beam-config / beam-log 的接口与常量逐条核对
# §8：递归查找 benches/、tests/、fuzz/ 目录
# §2：文档树形图 29 个路径逐个 Test-Path
```

---

## 11. 汇总待办（各分片自评的前 3 条）

以下由各分片审计员独立给出，再按影响面归并。**同一条目可能出现在多个分片** —— 例如「控制面未接通」同时出现在 Part 1（协议通道）、Part 4（session 控制循环）、Part 6（桌面端 connect）。

### 4. beam-proto 与协议规格

1. **§5 三通道与帧头语义对齐**（P5-02、P5-03、P5-25）：协议类型已经齐备，但控制流/输入流在 `crates/beam-client/src/adapters.rs:82-99` 是空实现、`recv_control`/`recv_frame` 直接返回 `None`，端到端跑不通；同时 `shard_idx` 实现为帧内全局序号而文档写「组内分片号」（`crates/beam-net/src/datagram.rs:129`），必须现在定稿，否则两端实现会互不兼容。
2. **§3.1 的测试三缺**（P1-21、P1-23、P1-24）：没有基准就无法证明「单消息编解码 < 2 µs」，没有 fuzz 就没有解码器健壮性保障，没有快照就无法保证线上格式不被无意改动。建议最少补三件：`cargo-fuzz`（或 proptest 的退化/对抗属性）目标、一个固定线上字节序列的快照测试、一个编解码基准（criterion 或 divan）。
3. **§6 时钟同步与 JNI 缓冲池未实现**（P6-03、P6-06）：`ClockSync{t0..t3}` 只有消息定义，全仓库没有任何求偏移的算法（已搜索 `clock_offset`/`offset_us`/`fn .*offset` 等），E2E 延迟无法据此换算；JNI DirectByteBuffer 池在 `crates/beam-ffi/src/lib.rs:46-47` 被明确推迟到 M1，热路径仍是切片拷贝。

### 5. beam-net / beam-pair / beam-discovery

1. **§3.2 线程模型整体未落地**：发送路径没有专用任务也没有 UDP GSO 批量发包（N-12），接收重组内联在调用方 future（N-13），FEC 解码留在接收任务内（N-14），而这三项正是 T1/T2/T3 延迟预算的载体；叠加完全没有基准测试（N-24），当前「能跑通」并不等于「达标」。
2. **v1 的抖动缓冲完全缺失，MVP 的 FEC 反而默认开启**：jitter_frames 只有配置字段没有任何缓冲实现（N-22），Wi-Fi 场景没有抖动吸收；FEC 则无 feature 门控、默认 0.2 恒开（N-28），在文档定义的 MVP 阶段白占带宽。
3. **三个文档错误码没有生产路径**：E2002 对端证书拒绝现在报 E2008（N-16）、E2004 帧过期从不被产生（N-18）、E3004 只是塞进 rustls 错误的字符串（P-18）。§3.11 要求「UI 只根据 code 显示文案」，这三条会让 UI 拿到错误或拿不到的 code。

### 6. 平台层 crate（capture/encode/audio/input/decode/render）

1. **打通一条真实端到端的 GPU 帧链路（capture → encode → decode → render），并把 `#![forbid(unsafe_code)]` 改成局部 `deny` + 白名单。**
2. **把「后端未启用」和「功能未实现」区分开，别让占位以「E5xxx/E6xxx/E7xxx/E1xxxx 后端未启用」的形式冒充已完成。**
3. **补上配置项的最后一公里，并把 `beam-config` 缺的三项补齐。**

### 7. session / host / client / ffi / gamestream

1. **把 §4.1 的时序落到 session 的控制面**：`Transport::recv_control` 全仓无调用点，`Hello/HelloAck/PairRequest/PairOk/RequestIdr/Feedback/Resume/Bye` 在 session/host/client/ffi 内零处理（见 T-14～T-23、S-04）。最小可行路径：在 `ClientRunner`/`HostRunner` 增加控制消息处理循环，用 `beam_proto::negotiate`（`crates/beam-proto/src/caps.rs:214`）完成能力协商，并在 `SessionInput::Negotiated` 之前真正跑一次协商。
2. **让 `stall_threshold=3s` / `reconnect_timeout=30s` 从常量变成判定**（S-19、T-08、T-09）：在 session 内记录「最后收帧时间」，超时进入 `Reconnecting` 并在 30 s 内尝试重建传输（配合 `Resume{session_id}`，T-21），否则 `Closed`。这是 §3.9「重连（v1）」与 §4.1 状态机的核心缺口，目前两个阈值完全没有消费方。
3. **打通 `beam-ffi` 的导出与热路径**（F-05、F-07、F-08、F-09）：`connect` 当前丢弃 `cfg`/`sink` 并必然失败；DirectByteBuffer 池只有设计文档。最低限度应让 `connect` 走 `ClientSession::connect_with`（注入传输 + 把帧转给 `FrameSink`），并为 uniffi 导出补 `#[uniffi::export]`/UDL 与 `build.rs`，同时解决 `#![forbid(unsafe_code)]`（`crates/beam-ffi/src/lib.rs:53`）与 JNI 零拷贝的冲突。

### 8. Android 12 模块

1. **打通 uniffi 产物链路，否则 Android 串流整体不可用**（AU-01、AB-04、AU-13）。`crates/beam-ffi` 目前**没有任何 uniffi 导出面**（无 `setup_scaffolding!`/`#[uniffi::export]`、无 `.udl`、无 `crate-type=["cdylib"]`），`libbeam_ffi.so` 与生成的 Kotlin 绑定均不存在，`android/` 内也没有 cargo-ndk 任务。`NativeBeamBridge` 的反射层写得再完整也永远只能落到 `UnavailableBeamClient`。最小修复顺序：① `crates/beam-ffi` 加 uniffi 导出（UDL 或 proc-macro）+ `crate-type = ["cdylib"]`；② `core-net/build.gradle.kts` 按 `core-net/README.md:83-94` 落 Exec 任务产出 `jniLibs/{arm64-v8a,x86_64}/libbeam_ffi.so`；③ 在 so 内起 tokio 运行时（AU-13）。
2. **补齐「会话可用」的最后一公里：输入采集→回传 + 500 ms 统计节拍**（PP-03、AU-10）。`InputMapper` 与 `MotionActions` 已就绪但无人调用，`StreamViewModel.sendInput`（`:201`）也无调用方 —— 触控/手柄/遥控事件从未上行主机；`session://stats` 的 500 ms 节拍在 Kotlin 与 Rust 两侧都没有产生者，`stats` 流恒空，统计浮层与性能面板拿不到数据。修这两条后「能连上」才等于「能用」。
3. **修复性能门禁与性能面板的可验证性**（AN-25、AN-23、AU-16）。`:core-data`/`:core-net` 的 perf 类型在 main 源集，会进 release APK，而 CI 的 `grep -qi "feature-perf\|perfstats"`（`.github/workflows/ci.yml:103`）匹配不到 `dev/beam/core/data/perf/*`，§7:276 的「release 无 perf 类」门禁形同虚设；同时 `PerfRecorder` 无调用方、`Routes.PERF` 无入口、解码 T1/T2 无任何验收代码。建议把 perf 模型整体移入 debug-only 源集、把 CI grep 改为 `dev/beam/core/data/perf\|dev/beam/core/net/Perf\|feature-perf`，并补一条解码耗时的测量与阈值断言。

### 9. Tauri 桌面端

1. **Windows 客户端的「画面」整条链路缺失**：桌面端只有一个 WebView 窗口，客户端画面区是 CSS 占位；`connect` 只做到 QUIC/TLS 握手探针就立即关连接；`beam-render`/`beam-decode` 未接入桌面端依赖。这是产品方案第 45 行与 §3.10 的直接验收项，也连带导致 EV-06（重连）、UX-38（保留最后一帧）、UX-35 永远无法真正落地。
2. **真实后端上配对请求链路走不通**：`CrateService.pending_pairs` 只有初始化为空的 `retain`/`iter`，从无写入点 → `pair://request` 永不带来内容、`reject_pair` 对任意 id 恒报 `E9101`、`approve_pair` 落库时设备名/平台回落「未知设备」。产品方案第 102 行把「主机端人工确认后写入信任库」列为不可省略的安全环节，当前实现满足不了。
3. **perf-stats 未接入，两个 debug command 与 `perf://tick` 是空壳**：`beam-perf` crate 存在但不在桌面端依赖里，`perf_snapshot` 在默认构建返回空、`perf_export` 恒 `E11003`、`perf://tick` 事件永不发出（且 §11 要求「隐藏入口仅 debug 构建存在」，侧栏诊断入口在 release 也常驻）。

---

## 10. 附录：核验方式

### A. 引用真实性

每个条目的 `file:line` 引用都用脚本逐条校验：**文件必须存在，且行号必须在文件实际行数范围内**。

```powershell
python verify_refs.py 'C:\Users\Wisdplat_ZC_0044\Documents\work\beam' part-*.md
```

### B. 主会话独立复核的高影响结论

下列结论由主会话用命令重新验证过一遍，不依赖审计员自述：

| 结论 | 复核方式 | 结果 |
|---|---|---|
| §4 分层规则合规 | 逐 crate 解析 Cargo.toml 的 workspace 依赖并与 mermaid 图比对 | L1 间仅 `beam-net -> beam-pair`；ffi 无平台 crate；session 仅依赖 proto ✓ |
| 平台后端被 `forbid(unsafe_code)` 封死 | 扫描 18 个 crate 的 `lib.rs` | **18/18 全部** `#![forbid(unsafe_code)]`，含 5 个需要 D3D11/COM 的平台 crate |
| 原生依赖声明了但没接线 | 扫描 `src/**` 里的 `windows_capture::` / `ffmpeg_next::` / `opus::` / `vigem_client::` | windows_capture **0**、ffmpeg_next **0**、opus **0**、vigem_client **2**（仅 ViGEmBus 检测，未建手柄） |
| §3.5/§3.6 五个配置项缺失 | 搜 `backend` / `display_id` / `min_update_interval` / `slices` / `sfe` 于 beam-config | **全部不存在** |
| perf 体系零接入 | 搜 `beam-perf` 于所有 Cargo.toml、搜 `perf_span!` 调用点 | 除 beam-perf 自身外**无任何 crate 依赖**，宏只在自身内部使用 |
| 控制面未接通 | 搜 `send_control` / `recv_control` 全部出现处 | 全是 trait 定义与**空实现**（参数 `_msg` 未使用），无任何驱动循环的调用点 |
| `stall_threshold` / `reconnect_timeout` 无消费方 | 搜两个常量在 beam-session 的全部出现处 | 只被声明、默认值化、赋值，**没有任何比较点** |
| `beam-ffi::connect` 丢弃 cfg 与 sink | 读 `crates/beam-ffi/src/client.rs` | `let _ = (cfg, sink);` 后用 `ClientSessionConfig::default()` 开会话 |
| §5 帧头逐位精确 | 读 `crates/beam-proto/src/frame.rs` 的 encode/decode | out[0]=(ver&0xF)<<4\|kind&0xF、out[10..16]=capture_ts 48bit 大端、FRAME_HEADER_LEN=16 ✓ |
| §3.14 五层优先级 | 读 `crates/beam-config/src/model.rs` | 五层 `cli/env/session/file/device` 齐备且 merge 顺序与文档一致；但 `device` 层**无生产代码填充** |
| Android flavor 缺失 | 搜 `flavorDimensions` / `productFlavors` 于 android/app/build.gradle.kts | **命中 0**，只有 buildType |
| **CI 的 perf 门禁检查对象错了，恒通过** | 用 CI 的 `unzip -l \| grep -i "feature-perf\|perfstats"` 模式扫 release APK 的 zip 条目名 | release APK 有 419 个条目（`classes.dex`/`lib/*.so`/`res/**`），**条目名里根本没有类名** → 0 命中，门禁无论构建成什么样都通过。改扫 dex 字符串后：debug 4 类命中、release **0 命中**（R8 已剥离，即当前结果合规但门禁没在守）。另：R8 `mapping.txt` **含有** `dev.beam.core.data.perf.PerfSnapshotStore` 等名字，说明这些类被 R8 处理后移除，故判据应统一为 dex 扫描 |
| 无性能基准 / 无 fuzz / 仅 1 个集成测试 | 递归找 `benches/`、`fuzz/`、`tests/` 目录 | benches **0**、fuzz **0**、集成测试仅 `beam-proto/tests/roundtrip.rs` |
| `beam-ffi` 无 uniffi 导出面 | 搜 `uniffi::` / `derive(Uniffi)` / `setup_scaffolding` / `build.rs` | **0 命中**，且 `crates/beam-ffi/build.rs` 不存在 —— 即使现在跑 uniffi-bindgen 也生成不出 Kotlin 绑定 |
| Android `InputMapper` 无生产调用方 | 搜全 `android/**/*.kt` 的 `InputMapper` | 仅命中自身定义与单测，**0 个生产调用点** → 触控/手柄/遥控采集→回传主机整条链未接线 |
| Android 缺 WOL 与全屏沉浸 | 搜 `WOL`/`Wake`/`MagicPacket` 与 `immersive` | 均 **0 命中**（产品方案 §7 `:feature-discovery` 要求 WOL） |
| `feature-stream` 越界 | 看 `android/feature-stream/src/main/java/dev/beam/feature/stream/StreamViewModel.kt:10-13,88` | import 了具体 `MediaCodecVideoDecoder` 并构造 `VideoFormat` → 触碰 §3.12「禁止：解码细节」 |
| 桌面端浅色状态色有、Android 端没有 | 读 `android/core-data/.../BeamTokens.kt` 与 `apps/desktop/src/styles.css` | 桌面端 `:root` 含 `#16a34a/#d97706/#dc2626`；Android `BeamTokens` **只有** `22C55E/F59E0B/EF4444`，浅色模式复用深色状态色 |

### C. 审计基线

审计开始时仓库的构建状态（用于判断「占位」与「构建失败」的区别）：

```
cargo fmt --all --check                    exit 0
cargo check --workspace --all-targets      exit 0, 0 warning
cargo test --workspace                     171 passed / 0 failed
gradlew --rerun-tasks test                 14 套件 / 92 次执行 / 0 失败
gradlew assembleDebug assembleRelease     BUILD SUCCESSFUL
npm run build                              exit 0, 81 modules
```

**结论：所有分片的「占位」与「缺失」都不是构建失败导致的，而是功能本身没写。**