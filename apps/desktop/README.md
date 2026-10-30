# Beam 串流 · Windows 桌面端

Beam 的 Windows 主机端 / 客户端外壳,技术栈为 **Tauri 2 + React 19 + TypeScript + Vite**。

对应技术设计文档 `docs/beam-tech-design.md` 的 §2 工程结构与 §3.11 `host-app / client`:
UI、托盘、通知都在这一层实现,只通过 **commands / events** 与 Rust 交互,视频帧不经 IPC。

---

## 目录结构

```
apps/desktop/
├─ package.json / package-lock.json
├─ vite.config.ts / tsconfig.json / tsconfig.node.json
├─ index.html
├─ scripts/gen-icons.mjs        # 生成 Tauri 图标(PNG/ICO/ICNS),零第三方依赖
└─ src/
   ├─ main.tsx                   # 入口
   ├─ App.tsx                    # 外壳:导航 + 8 个事件订阅 + 主题
   ├─ styles.css                 # 设计令牌(深/浅色)+ 组件样式
   ├─ ipc/                       # 与 Rust 的唯一通道
   │  ├─ types.ts                # DTO,与 src-tauri/src/dto.rs 一一对应
   │  ├─ errors.ts               # {code,message,retryable} → 中文文案
   │  ├─ commands.ts             # 14 个 command 的类型化封装
   │  └─ events.ts               # 8 个事件的类型化订阅
   ├─ stores/                    # zustand 状态管理
   │  ├─ hostStore.ts            # 主机状态
   │  ├─ deviceStore.ts          # 设备 / 配对 / 发现
   │  ├─ settingsStore.ts        # 设置
   │  ├─ sessionStore.ts         # 客户端会话与统计
   │  ├─ perfStore.ts            # 性能统计(仅 Debug)
   │  └─ toastStore.ts           # 轻量提示
   ├─ i18n/                      # 中文优先,预留 en
   │  ├─ zh-CN.ts  en.ts  index.tsx
   ├─ components/                # 可复用 UI
   │  ├─ Sidebar.tsx  Card.tsx  SegmentedControl.tsx  Toggle.tsx
   │  ├─ StatusPill.tsx  Feedback.tsx  ToastHost.tsx  BackendNotice.tsx
   │  ├─ QrCode.tsx  PairingPanel.tsx  DeviceRows.tsx
   │  └─ Charts.tsx  LatencyOverlay.tsx
   └─ pages/                     # 五个页面
      ├─ HomePage.tsx            # 主机
      ├─ DevicesPage.tsx         # 设备
      ├─ SettingsPage.tsx        # 设置
      ├─ ConnectPage.tsx         # 连接其他主机
      └─ PerfPage.tsx            # 诊断(Debug)
```

`src-tauri/` 严格按 §2 的文件名划分:`main.rs`、`commands.rs`、`events.rs`、`tray.rs`,
另有 `dto.rs`(共享结构)、`error.rs`(错误模型)、`service.rs`(服务接缝)。

---

## 开发命令

```bash
cd apps/desktop

npm install          # 安装前端依赖
npm run dev          # 只跑前端(Vite,端口 1420)
npm run tauri:dev    # 完整桌面端开发(Vite + Rust,带热重载)
npm run build        # 前端生产构建(tsc --noEmit + vite build)
npm run typecheck    # 只做类型检查
npm run tauri:build  # 打包 Windows 安装包

cd src-tauri
cargo check          # Rust 侧类型检查
cargo build          # Rust 侧编译
```

图标:`npm run icons` 会重新生成 `src-tauri/icons/` 下的 PNG / ICO / ICNS。

---

## §3.11 命令与事件对照

### Command(后端 `src-tauri/src/commands.rs`)

| §3.11 命令 | 前端调用 | 说明 |
|---|---|---|
| `get_host_status` | `commands.getHostStatus()` | 主机状态快照 |
| `start_hosting(opts)` | `commands.startHosting(opts)` | 一键开播,成功广播 `host://state-changed` |
| `stop_hosting` | `commands.stopHosting()` | 停止串流,成功广播 `host://state-changed` |
| `get_pairing_code` | `commands.getPairingCode()` | 4 位 PIN + 二维码载荷 |
| `approve_pair(id)` | `commands.approvePair(id)` | 同意配对 |
| `reject_pair(id)` | `commands.rejectPair(id)` | 拒绝配对 |
| `remove_device(id)` | `commands.removeDevice(id)` | 移除已配对设备 |
| `list_hosts` | `commands.listHosts()` | 局域网发现到的主机 |
| `connect(host_id)` | `commands.connect(hostId)` | 连接主机,建立客户端会话 |
| `disconnect` | `commands.disconnect()` | 断开会话,广播 `session://disconnected` |
| `get_settings` | `commands.getSettings()` | 设置全量 |
| `set_settings(patch)` | `commands.setSettings(patch)` | 设置增量(§3.14 优先级合并) |
| `perf_snapshot` *(Debug)* | `commands.perfSnapshot()` | 性能统计快照 |
| `perf_export(format)` *(Debug)* | `commands.perfExport(format)` | 导出 CSV / JSON |

**补充的 2 个只读 command**(§3.11 清单外):`list_devices`、`list_pair_requests`。
§3.11 的 12 个命令里没有读取「已配对设备列表」和「待确认配对请求」的入口,
而「设备」页必须展示它们,因此补了两个纯读取 command。

### Event(后端 `src-tauri/src/events.rs`)

| §3.11 事件 | 频率 | 前端订阅 |
|---|---|---|
| `host://state-changed` | 状态变化时 | `onHostStateChanged` → `hostStore` |
| `pair://request` | 收到新配对请求时 | `onPairRequest` → `deviceStore` |
| `pair://code-rotated` | PIN 轮换时(120 s) | `onPairCodeRotated` → `deviceStore` |
| `discovery://updated` | 发现列表刷新时 | `onDiscoveryUpdated` → `deviceStore` |
| `session://stats` | 500 ms | `onSessionStats` → `sessionStore` |
| `session://reconnecting` | 进入重连时 | `onSessionReconnecting` → `sessionStore` |
| `session://disconnected` | 会话结束时 | `onSessionDisconnected` → `sessionStore` |
| `perf://tick` | 250 ms(仅 Debug) | `onPerfTick` → `perfStore` |

---

## 错误约定

后端 command 失败时返回:

```json
{ "code": "E2001", "message": "connection timeout", "retryable": true }
```

前端**按 `code` 查表显示中文文案**(`src/ipc/errors.ts` 的 `ERROR_TEXT`),
`message` 只用于日志与调试。错误码沿用文档分段:

| 段 | 含义 | 示例 |
|---|---|---|
| `E1xxx` | proto | `E1001` 版本不兼容 |
| `E2xxx` | net | `E2001` 连接超时、`E2002` 证书拒绝 |
| `E3xxx` | pair | `E3001` PIN 错误、`E3002` 次数超限锁定 |
| `E4xxx` | discovery | `E4001` 局域网搜索失败 |
| `E5xxx` | capture | `E5001` 无显示器、`E5002` 独占全屏 |
| `E6xxx` | encode | `E6001` 无可用硬编、`E6003` 参数不支持降档 |
| `E9xxx` | session | `E9001` 状态不允许、`E9003` 重连失败 |
| `E11xxx` | config | `E11001` 设置非法、`E11003` perf 仅 Debug |
| `E12xxx` | 应用外壳 | `E12001` 未知命令 |

---

## 与 `crates/` 的接线状态

§4 分层要求 `apps/desktop → beam-host / beam-client`。本目录通过
**`src-tauri/src/service.rs` 的 `BeamService` trait** 作为接缝:

- `BeamService` —— 与 crate 无关的 trait,14 个 command 与 8 个事件全部只依赖它。
- `LocalService` —— 当前默认实现,一个本地内存实现,让桌面端在没有 `crates/` 时也能完整跑通交互。

`src-tauri/Cargo.toml` 里预留了 `beam-crates` feature 与五行路径依赖(默认注释):

```toml
[features]
beam-crates = ["dep:beam-host", "dep:beam-client", "dep:beam-session",
               "dep:beam-config", "dep:beam-log"]

[dependencies]
beam-host    = { path = "../../crates/beam-host",    optional = true }
beam-client  = { path = "../../crates/beam-client",  optional = true }
beam-session = { path = "../../crates/beam-session", optional = true }
beam-config  = { path = "../../crates/beam-config",  optional = true }
beam-log     = { path = "../../crates/beam-log",     optional = true }
```

> ⚠️ Cargo 在**解析阶段**就会读取 optional 路径依赖的 manifest,
> 所以在 `crates/beam-*` 落地之前不能直接启用,否则会报
> `failed to load manifest for dependency`。
> crate 落地后:取消注释上面五行 → 把 feature 改成上面的写法 →
> 新增一个实现 `BeamService` 的适配器 → `cargo check --features beam-crates`。

### 客户端真实上屏（§3.10 / 产品方案第 45 行）

`client-video` feature 打开**客户端**的真实解码 + 上屏:

```toml
client-video = ["beam-crates", "beam-client/video"]
```

它同时打开:

| crate 的 feature | 打开的东西 | 需要什么 |
|---|---|---|
| `beam-render/d3d11` | 独立原生窗口 + D3D11 flip-model 交换链 | Windows |
| `beam-decode/ffmpeg` | D3D11VA 硬解（HEVC / H.264） | LGPL ffmpeg 开发库（`FFMPEG_DIR`,§7） |

**§4 分层**:桌面端**不直接**依赖 `beam-decode` / `beam-render`,
只经由 `beam-client`（L3）拿到已经建好的窗口与交换链
（`beam_client::VideoWindow` / `ClientRuntime::connect_to_window`）。

没开这个 feature 时,客户端会话**照常建立**（控制面、配对、输入回传都在),
但**画面不会上屏** —— 这一点在日志里如实说明(`E10014`),
不会伪装成「在串流」。

本机自检（需要 `video` feature 与 ffmpeg）:

```powershell
$env:FFMPEG_DIR = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
cargo test -p beam-client --features video --test video_present -- --ignored
```

该自检验证的是「独立原生窗口 + D3D11 交换链 + 硬解上下文」三步都真的建得起来;
**「画面真的出现在窗口里」属于端到端验收**,需要一条真实的主机串流链路,未在本机自动验证。

### 当前后端是桩实现,界面会明确标注

`LocalService` 里,**主机名、本机 IPv4、端口、设置项是真实取值**;
而**已配对设备列表、局域网发现列表、延迟统计是合成的演示数据**。
后端通过 `HostStatus.backend = "stub"` 告知前端,界面顶部会显示
「本地桩实现 / 演示数据」提示,避免被误读为实测结果。

---

## 构建注意事项

- **Rust 版本**:本机 Rust 1.86.0。`tauri 2.12+` / `tauri-utils 2.10+` 的 MSRV 是 1.90,
  因此 `Cargo.toml` 锁定 `tauri ~2.11.6` / `tauri-build ~2.6.3`,
  并在 `src-tauri/.cargo/config.toml` 里开启 `resolver.incompatible-rust-versions = "fallback"`,
  让 Cargo 自动挑选 MSRV ≤ 1.86 的传递依赖。
- **src-tauri 是独立工作区**:`src-tauri/Cargo.toml` 内含 `[workspace]` 表,
  仓库根 `Cargo.toml` 也 `exclude = ["apps/desktop"]`,两边互不干扰。
- **`npm install` 依赖平台二进制**:`@tauri-apps/cli` 通过 npm optionalDependencies
  拉取 `@tauri-apps/cli-win32-x64-msvc`,不依赖 GitHub 二进制源。
- **`cargo check` 需要前端产物**:`tauri::generate_context!()` 会校验 `frontendDist`,
  所以请先执行 `npm run build`(或 `npm run tauri:dev`)。
- **Windows 运行依赖**:需要 WebView2(Win10/11 自带)与 MSVC 工具链。

---

## 许可

自有代码 MIT OR Apache-2.0。净室要求:不复制、不翻译、不改写 Sunshine / Moonlight 等 GPL 项目的代码(§7)。
