/**
 * 设置 store(§3.14)。
 *
 * 状态来源:`get_settings` 读全量,`set_settings(patch)` 做增量更新。
 */

import { create } from 'zustand'

import { commands } from '../ipc/commands'
import { toBeamError, type BeamError } from '../ipc/errors'
import type { Settings, SettingsPatch } from '../ipc/types'

interface SettingsStoreState {
  settings: Settings | null
  loading: boolean
  saving: boolean
  error: BeamError | null
  load: () => Promise<void>
  patch: (patch: SettingsPatch) => Promise<boolean>
}

export const useSettingsStore = create<SettingsStoreState>((set) => ({
  settings: null,
  loading: false,
  saving: false,
  error: null,

  load: async () => {
    set({ loading: true })
    try {
      const settings = await commands.getSettings()
      set({ settings, error: null })
    } catch (raw) {
      set({ error: toBeamError(raw) })
    } finally {
      set({ loading: false })
    }
  },

  patch: async (patch) => {
    set({ saving: true })
    try {
      const settings = await commands.setSettings(patch)
      set({ settings, error: null })
      return true
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return false
    } finally {
      set({ saving: false })
    }
  },
}))

/** 便捷读取:当前设置(可能为 null)。 */
export function useSettings(): Settings | null {
  return useSettingsStore((state) => state.settings)
}
