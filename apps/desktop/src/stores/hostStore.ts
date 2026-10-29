/**
 * 主机状态 store。
 *
 * 状态来源:
 * - 初始:`get_host_status`(§3.11)。
 * - 增量:`host://state-changed` 事件(§3.11)。
 */

import { create } from 'zustand'

import { commands } from '../ipc/commands'
import { toBeamError, type BeamError } from '../ipc/errors'
import type { HostStatus, StartHostingOptions } from '../ipc/types'

interface HostStoreState {
  status: HostStatus | null
  loading: boolean
  busy: boolean
  error: BeamError | null
  /** 读取主机状态(页面挂载 / 手动刷新)。 */
  load: () => Promise<void>
  /** 一键开播(§3.11 `start_hosting`)。 */
  start: (opts?: StartHostingOptions) => Promise<boolean>
  /** 停止串流(§3.11 `stop_hosting`)。 */
  stop: () => Promise<boolean>
  /** 接收 `host://state-changed` 事件。 */
  applyStatus: (status: HostStatus) => void
  setError: (error: BeamError | null) => void
}

export const useHostStore = create<HostStoreState>((set) => ({
  status: null,
  loading: false,
  busy: false,
  error: null,

  load: async () => {
    set({ loading: true })
    try {
      const status = await commands.getHostStatus()
      set({ status, error: null })
    } catch (raw) {
      set({ error: toBeamError(raw) })
    } finally {
      set({ loading: false })
    }
  },

  start: async (opts) => {
    set({ busy: true })
    try {
      const status = await commands.startHosting(opts ?? {})
      set({ status, error: null })
      return true
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return false
    } finally {
      set({ busy: false })
    }
  },

  stop: async () => {
    set({ busy: true })
    try {
      const status = await commands.stopHosting()
      set({ status, error: null })
      return true
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return false
    } finally {
      set({ busy: false })
    }
  },

  applyStatus: (status) => set({ status }),
  setError: (error) => set({ error }),
}))

/** 便捷选择器:当前是否正在串流(含「已开播等待连接」)。 */
export function selectIsHosting(status: HostStatus | null): boolean {
  return status?.state === 'hosting' || status?.state === 'streaming'
}
