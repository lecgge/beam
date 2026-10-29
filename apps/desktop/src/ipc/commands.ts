/**
 * command 封装层。
 *
 * §3.11 的 12 个 command + Debug 下的 `perf_snapshot` / `perf_export`,
 * 加上 2 个补充只读入口(`list_devices` / `list_pair_requests`)。
 *
 * 统一约定:成功直接返回值;失败统一抛出 {@link BeamError}
 * (`{code, message, retryable}`),由 `errors.ts` 负责转成中文文案。
 */

import { invoke } from '@tauri-apps/api/core'

import { toBeamError } from './errors'
import type {
  Device,
  DiscoveredHost,
  HostStatus,
  PairingCode,
  PairRequest,
  PerfExportResult,
  PerfSnapshot,
  SessionInfo,
  Settings,
  SettingsPatch,
  StartHostingOptions,
} from './types'

/** 统一的 invoke 包装:把后端错误规整成 {@link BeamError} 后再抛。 */
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args)
  } catch (raw) {
    throw toBeamError(raw)
  }
}

export const commands = {
  // ---- §3.11 主机 ----
  /** §3.11 `get_host_status` */
  getHostStatus: () => call<HostStatus>('get_host_status'),

  /** §3.11 `start_hosting(opts)` */
  startHosting: (opts: StartHostingOptions = {}) =>
    call<HostStatus>('start_hosting', { opts }),

  /** §3.11 `stop_hosting` */
  stopHosting: () => call<HostStatus>('stop_hosting'),

  // ---- §3.11 配对 ----
  /** §3.11 `get_pairing_code` */
  getPairingCode: () => call<PairingCode>('get_pairing_code'),

  /** §3.11 `approve_pair(id)` */
  approvePair: (id: string) => call<Device>('approve_pair', { id }),

  /** §3.11 `reject_pair(id)` */
  rejectPair: (id: string) => call<void>('reject_pair', { id }),

  /** §3.11 `remove_device(id)` */
  removeDevice: (id: string) => call<void>('remove_device', { id }),

  // ---- §3.11 发现与连接 ----
  /** §3.11 `list_hosts` */
  listHosts: () => call<DiscoveredHost[]>('list_hosts'),

  /** §3.11 `connect(host_id)` */
  connect: (hostId: string) => call<SessionInfo>('connect', { hostId }),

  /** §3.11 `disconnect` */
  disconnect: () => call<void>('disconnect'),

  // ---- §3.11 设置 ----
  /** §3.11 `get_settings` */
  getSettings: () => call<Settings>('get_settings'),

  /** §3.11 `set_settings(patch)` */
  setSettings: (patch: SettingsPatch) => call<Settings>('set_settings', { patch }),

  // ---- §3.11 Debug 专属 ----
  /** §3.11 `perf_snapshot`(Release 构建返回 E11003) */
  perfSnapshot: () => call<PerfSnapshot>('perf_snapshot'),

  /** §3.11 `perf_export(format)`:`csv` / `json` */
  perfExport: (format: 'csv' | 'json') => call<PerfExportResult>('perf_export', { format }),

  // ---- 补充只读入口(§3.11 清单外) ----
  /** 已配对设备列表。 */
  listDevices: () => call<Device[]>('list_devices'),

  /** 待确认的配对请求。 */
  listPairRequests: () => call<PairRequest[]>('list_pair_requests'),
}

export type Commands = typeof commands
