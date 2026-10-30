/**
 * 配对尝试计数(§3.3 `max_attempts=3`)。
 *
 * 桌面端作为**客户端**连接一台「未配对」的主机时,需要输入对方屏幕上的
 * 4 位 PIN(§3.3 `pin_len=4`)。§3.3 的策略是最多试 3 次、超出即锁定 60 秒
 * (与 crates/beam-pair 的 `PairingPolicy` 一致),这里在前端维护剩余次数,
 * 让错误态能显示「剩余 N 次」(§16 状态行:错误 = 描红 + 剩余次数)。
 *
 * 说明:后端的 `connect` command 目前**没有 PIN 参数**(Rust 侧不在本目录
 * 范围内),所以 PIN 的最终校验仍在后端;前端负责格式校验与尝试预算,
 * 后端一旦返回 E3001 / E3002,同样消耗这里的预算。
 */

import { create } from 'zustand'

/** §3.3 `max_attempts=3`。 */
export const PIN_MAX_ATTEMPTS = 3

interface PairStoreState {
  /** 剩余可试次数。 */
  attemptsLeft: number
  /** 是否已锁定(超过 max_attempts 后锁 60 秒)。 */
  locked: boolean
  /** 记一次失败尝试,返回剩余次数。 */
  fail: () => number
  /** 配对成功后复位。 */
  reset: () => void
}

export const usePairStore = create<PairStoreState>((set, get) => ({
  attemptsLeft: PIN_MAX_ATTEMPTS,
  locked: false,

  fail: () => {
    const left = Math.max(0, get().attemptsLeft - 1)
    set({ attemptsLeft: left, locked: left === 0 })
    return left
  },

  reset: () => set({ attemptsLeft: PIN_MAX_ATTEMPTS, locked: false }),
}))
