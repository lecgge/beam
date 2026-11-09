/**
 * 前后端共享的类型定义。
 *
 * 与 `src-tauri/src/dto.rs` 一一对应:后端所有结构都 `#[serde(rename_all = "camelCase")]`,
 * 所以这里的字段名与 JSON 完全一致。
 */

/** 后端实现来源:`stub` = 本地桩实现(演示数据),`beamCrates` = 已接 crates/。 */
export type BackendKind = 'stub' | 'beamCrates'

/** §3.9 会话状态机的桌面端子集。 */
export type HostState = 'idle' | 'hosting' | 'streaming' | 'error'

export interface HostStatus {
  state: HostState
  deviceName: string
  localIp: string | null
  port: number
  gpu: string | null
  encoder: string | null
  codec: string | null
  resolution: string
  fps: number
  bitrateMbps: number
  discoveryEnabled: boolean
  connectedDevice: Device | null
  message: string | null
  backend: BackendKind
  revision: number
  updatedAtMs: number
}

export interface StartHostingOptions {
  resolution?: string
  fps?: number
  codec?: string
  bitrateMbps?: number
  virtualDisplay?: boolean
  gamepad?: boolean
  audioMode?: string
}

export interface PairingCode {
  code: string
  qrPayload: string
  expiresInS: number
  rotatedAtMs: number
}

/**
 * 待确认请求的方向(§3.3)。
 *
 * - `host`：有客户端要连**本机**(本机是主机)。
 * - `client`：我们要连的那台主机还没进过本机的信任库。
 */
export type PairDirection = 'host' | 'client'

export interface PairRequest {
  id: string
  name: string
  platform: string
  address: string
  fingerprint: string
  requestedAtMs: number
  direction: PairDirection
}

export interface DevicePerms {
  view: boolean
  keyboard: boolean
  mouse: boolean
  gamepad: boolean
}

export interface LastSession {
  at: string
  resolution: string
  fps: number
  avgLatencyMs: number | null
}

export interface Device {
  id: string
  name: string
  platform: string
  fingerprint: string
  pairedAt: string
  online: boolean
  streaming: boolean
  lastSession: LastSession | null
  perms: DevicePerms
}

export interface DiscoveredHost {
  id: string
  name: string
  address: string
  port: number
  trusted: boolean
  version: string
  gpu: string | null
  online: boolean
  /** 对端平台（§3.4 mDNS TXT 的 `plat`；缺失时为「未知平台」）。 */
  platform: string
}

export interface SessionInfo {
  sessionId: string
  hostId: string
  hostName: string
  state: string
  resolution: string
  fps: number
  codec: string
  startedAtMs: number
}

export interface SessionStats {
  sessionId: string
  e2eMs: number
  captureMs: number
  colorConvertMs: number
  encodeMs: number
  networkMs: number
  decodeMs: number
  renderMs: number
  fps: number
  bitrateMbps: number
  lossPct: number
  fecRecoveredPct: number
  jitterMs: number
  droppedFrames: number
  resolution: string
  codec: string
  timestampMs: number
}

export interface ReconnectingInfo {
  sessionId: string
  elapsedMs: number
  keepLastFrame: boolean
}

export interface DisconnectedInfo {
  sessionId: string
  reason: string
  code: string | null
}

export interface Settings {
  // 画质
  resolution: string
  fps: number
  bitrateMbps: number
  hdr: boolean
  matchClientResolution: boolean
  // 编码
  encoder: string
  codec: string
  preset: string
  captureBackend: string
  slices: number
  sfe: boolean
  // 网络
  fecRatio: number
  port: number
  allowTailscale: boolean
  discoveryEnabled: boolean
  bitrateMinMbps: number
  bitrateMaxMbps: number
  // 音频
  audioMode: string
  audioBitrateKbps: number
  // 界面
  language: string
  theme: string
  showStatsOverlay: boolean
  minimizeToTray: boolean
  launchOnStartup: boolean
  // 输入(§3.8)：是否把本机键鼠回传给主机
  shareInput: boolean
}

/** `set_settings(patch)` 的增量载荷:只带需要改的字段。 */
export type SettingsPatch = Partial<Settings>

export interface PerfStageStat {
  stage: string
  meanMs: number
  p50Ms: number
  p99Ms: number
}

export interface PerfMetric {
  name: string
  value: string
  unit: string
}

export interface PerfSnapshot {
  available: boolean
  stages: PerfStageStat[]
  metrics: PerfMetric[]
  e2eSeries: number[]
  samples: number
  capturedAtMs: number
}

export interface PerfTick {
  e2eMs: number
  fps: number
  bitrateMbps: number
  cpuPct: number
  gpuPct: number
  memoryMb: number
  timestampMs: number
}

export interface PerfExportResult {
  format: string
  path: string
  bytes: number
  rows: number
}
