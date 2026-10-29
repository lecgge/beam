/**
 * 设备 / 配对 / 发现 store。
 *
 * 状态来源:
 * - 初始:`list_devices` / `list_pair_requests` / `list_hosts`。
 * - 增量:`pair://request`(新增配对请求)、`discovery://updated`(发现列表刷新)。
 * - 变更后(同意/拒绝/移除)由命令返回值 + 主动重拉刷新。
 */

import { create } from 'zustand'

import { commands } from '../ipc/commands'
import { toBeamError, type BeamError } from '../ipc/errors'
import type { Device, DiscoveredHost, PairingCode, PairRequest } from '../ipc/types'

interface DeviceStoreState {
  devices: Device[]
  pairRequests: PairRequest[]
  hosts: DiscoveredHost[]
  pairing: PairingCode | null
  loading: boolean
  busy: boolean
  error: BeamError | null

  loadDevices: () => Promise<void>
  loadPairRequests: () => Promise<void>
  loadPairing: () => Promise<void>
  loadHosts: () => Promise<void>
  loadAll: () => Promise<void>

  approve: (id: string) => Promise<Device | null>
  reject: (id: string) => Promise<boolean>
  remove: (id: string) => Promise<boolean>

  applyDevices: (devices: Device[]) => void
  applyPairRequests: (requests: PairRequest[]) => void
  addPairRequest: (request: PairRequest) => void
  applyHosts: (hosts: DiscoveredHost[]) => void
  applyPairing: (pairing: PairingCode) => void
  setError: (error: BeamError | null) => void
}

export const useDeviceStore = create<DeviceStoreState>((set, get) => ({
  devices: [],
  pairRequests: [],
  hosts: [],
  pairing: null,
  loading: false,
  busy: false,
  error: null,

  loadDevices: async () => {
    try {
      const devices = await commands.listDevices()
      set({ devices })
    } catch (raw) {
      set({ error: toBeamError(raw) })
    }
  },

  loadPairRequests: async () => {
    try {
      const pairRequests = await commands.listPairRequests()
      set({ pairRequests })
    } catch (raw) {
      set({ error: toBeamError(raw) })
    }
  },

  loadPairing: async () => {
    try {
      const pairing = await commands.getPairingCode()
      set({ pairing })
    } catch (raw) {
      set({ error: toBeamError(raw) })
    }
  },

  loadHosts: async () => {
    try {
      const hosts = await commands.listHosts()
      set({ hosts })
    } catch (raw) {
      set({ error: toBeamError(raw) })
    }
  },

  loadAll: async () => {
    set({ loading: true })
    await Promise.all([
      get().loadDevices(),
      get().loadPairRequests(),
      get().loadHosts(),
    ])
    set({ loading: false })
  },

  approve: async (id) => {
    set({ busy: true })
    try {
      const device = await commands.approvePair(id)
      // 同意后列表发生变化,重新拉一次保持一致。
      await get().loadDevices()
      await get().loadPairRequests()
      set({ error: null })
      return device
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return null
    } finally {
      set({ busy: false })
    }
  },

  reject: async (id) => {
    set({ busy: true })
    try {
      await commands.rejectPair(id)
      await get().loadPairRequests()
      set({ error: null })
      return true
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return false
    } finally {
      set({ busy: false })
    }
  },

  remove: async (id) => {
    set({ busy: true })
    try {
      await commands.removeDevice(id)
      await get().loadDevices()
      set({ error: null })
      return true
    } catch (raw) {
      set({ error: toBeamError(raw) })
      return false
    } finally {
      set({ busy: false })
    }
  },

  applyDevices: (devices) => set({ devices }),
  applyPairRequests: (requests) => set({ pairRequests: requests }),

  addPairRequest: (request) =>
    set((state) => ({
      pairRequests: state.pairRequests.some((item) => item.id === request.id)
        ? state.pairRequests
        : [...state.pairRequests, request],
    })),

  applyHosts: (hosts) => set({ hosts }),
  applyPairing: (pairing) => set({ pairing }),
  setError: (error) => set({ error }),
}))
