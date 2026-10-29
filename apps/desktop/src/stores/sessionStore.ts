/**
 * 客户端会话 store(§3.9)。
 *
 * 状态来源:
 * - `connect` / `disconnect` 命令返回。
 * - `session://stats`(500 ms)、`session://reconnecting`、`session://disconnected` 事件。
 */

import { create } from 'zustand'

import { commands } from '../ipc/commands'
import { toBeamError, type BeamError } from '../ipc/errors'
import type { ReconnectingInfo, SessionInfo, SessionStats } from '../ipc/types'

interface SessionStoreState {
  session: SessionInfo | null
  stats: SessionStats | null
  reconnecting: ReconnectingInfo | null
  connecting: boolean
  error: BeamError | null

  connect: (hostId: string) => Promise<boolean>
  disconnect: () => Promise<boolean>
  applyStats: (stats: SessionStats) => void
  applyReconnecting: (info: ReconnectingInfo) => void
  clearSession: () => void
  setError: (error: BeamError | null) => void
}

export const useSessionStore = create<SessionStoreState>((set) => ({
  session: null,
  stats: null,
  reconnecting: null,
  connecting: false,
  error: null,

  connect: async (hostId) => {
    set({ connecting: true })
    try {
      const session = await commands.connect(hostId)
      set({ session, error: null })
      return true
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return false
    } finally {
      set({ connecting: false })
    }
  },

  disconnect: async () => {
    try {
      await commands.disconnect()
      set({ session: null, stats: null, reconnecting: null, error: null })
      return true
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return false
    }
  },

  applyStats: (stats) => set({ stats }),
  applyReconnecting: (info) => set({ reconnecting: info }),
  clearSession: () => set({ session: null, stats: null, reconnecting: null }),
  setError: (error) => set({ error }),
}))
