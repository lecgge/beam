/**
 * 事件订阅层。
 *
 * §3.11 的 8 个事件:前端只通过事件被动接收后端推送,不轮询。
 * 每个订阅函数返回 `UnlistenFn`,组件卸载时调用即可解除订阅。
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import type {
  DiscoveredHost,
  HostStatus,
  PairingCode,
  PairRequest,
  PerfTick,
  ReconnectingInfo,
  SessionStats,
} from './types'

/** §3.11 事件名常量。 */
export const EVENTS = {
  /** 主机状态变化(开播 / 停止 / 设备变动)。 */
  hostStateChanged: 'host://state-changed',
  /** 收到新的配对请求。 */
  pairRequest: 'pair://request',
  /** 配对码轮换(§3.3 `pin_ttl=120s`)。 */
  pairCodeRotated: 'pair://code-rotated',
  /** 局域网发现列表刷新。 */
  discoveryUpdated: 'discovery://updated',
  /** 会话统计,500 ms 一拍。 */
  sessionStats: 'session://stats',
  /** 进入重连状态。 */
  sessionReconnecting: 'session://reconnecting',
  /** 会话结束。 */
  sessionDisconnected: 'session://disconnected',
  /** 性能单拍,250 ms 一拍,仅 Debug。 */
  perfTick: 'perf://tick',
} as const

export type EventName = (typeof EVENTS)[keyof typeof EVENTS]

/** §3.11 `host://state-changed` */
export function onHostStateChanged(
  handler: (payload: HostStatus) => void,
): Promise<UnlistenFn> {
  return listen<HostStatus>(EVENTS.hostStateChanged, (event) => handler(event.payload))
}

/** §3.11 `pair://request` */
export function onPairRequest(handler: (payload: PairRequest) => void): Promise<UnlistenFn> {
  return listen<PairRequest>(EVENTS.pairRequest, (event) => handler(event.payload))
}

/** §3.11 `pair://code-rotated` */
export function onPairCodeRotated(handler: (payload: PairingCode) => void): Promise<UnlistenFn> {
  return listen<PairingCode>(EVENTS.pairCodeRotated, (event) => handler(event.payload))
}

/** §3.11 `discovery://updated` */
export function onDiscoveryUpdated(
  handler: (payload: DiscoveredHost[]) => void,
): Promise<UnlistenFn> {
  return listen<DiscoveredHost[]>(EVENTS.discoveryUpdated, (event) => handler(event.payload))
}

/** §3.11 `session://stats`(500 ms) */
export function onSessionStats(handler: (payload: SessionStats) => void): Promise<UnlistenFn> {
  return listen<SessionStats>(EVENTS.sessionStats, (event) => handler(event.payload))
}

/** §3.11 `session://reconnecting` */
export function onSessionReconnecting(
  handler: (payload: ReconnectingInfo) => void,
): Promise<UnlistenFn> {
  return listen<ReconnectingInfo>(EVENTS.sessionReconnecting, (event) => handler(event.payload))
}

/** §3.11 `session://disconnected` */
export function onSessionDisconnected(
  handler: (payload: { sessionId: string; reason: string; code: string | null }) => void,
): Promise<UnlistenFn> {
  return listen<{ sessionId: string; reason: string; code: string | null }>(
    EVENTS.sessionDisconnected,
    (event) => handler(event.payload),
  )
}

/** §3.11 `perf://tick`(250 ms,仅 Debug) */
export function onPerfTick(handler: (payload: PerfTick) => void): Promise<UnlistenFn> {
  return listen<PerfTick>(EVENTS.perfTick, (event) => handler(event.payload))
}
