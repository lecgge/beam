/**
 * 性能统计 store(§3.13,仅 Debug)。
 *
 * 状态来源:
 * - `perf://tick` 事件(250 ms 一拍)。
 * - `perf_snapshot` / `perf_export` 命令。
 */

import { create } from 'zustand'

import { commands } from '../ipc/commands'
import { toBeamError, type BeamError } from '../ipc/errors'
import type { PerfExportResult, PerfSnapshot, PerfTick } from '../ipc/types'

interface PerfStoreState {
  tick: PerfTick | null
  snapshot: PerfSnapshot | null
  available: boolean
  exporting: boolean
  error: BeamError | null

  load: () => Promise<void>
  exportData: (format: 'csv' | 'json') => Promise<PerfExportResult | null>
  applyTick: (tick: PerfTick) => void
}

export const usePerfStore = create<PerfStoreState>((set) => ({
  tick: null,
  snapshot: null,
  available: false,
  exporting: false,
  error: null,

  load: async () => {
    try {
      const snapshot = await commands.perfSnapshot()
      set({ snapshot, available: snapshot.available, error: null })
    } catch (raw) {
      const error = toBeamError(raw)
      // Release 构建下后端返回 E11003:不是故障,而是「功能按设计不可用」。
      set({
        error,
        available: false,
        snapshot: error.code === 'E11003' ? null : null,
      })
    }
  },

  exportData: async (format) => {
    set({ exporting: true })
    try {
      const result = await commands.perfExport(format)
      set({ error: null })
      return result
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return null
    } finally {
      set({ exporting: false })
    }
  },

  applyTick: (tick) => set({ tick, available: true }),
}))
