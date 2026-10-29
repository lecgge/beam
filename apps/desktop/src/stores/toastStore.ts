/**
 * 轻量 toast store:操作结果与错误提示。
 */

import { create } from 'zustand'

export type ToastKind = 'info' | 'success' | 'error'

export interface Toast {
  id: number
  kind: ToastKind
  text: string
}

interface ToastStoreState {
  toasts: Toast[]
  push: (kind: ToastKind, text: string) => void
  dismiss: (id: number) => void
}

let sequence = 0

const TOAST_TTL_MS = 3600

export const useToastStore = create<ToastStoreState>((set) => ({
  toasts: [],

  push: (kind, text) => {
    const id = ++sequence
    set((state) => ({ toasts: [...state.toasts, { id, kind, text }] }))
    window.setTimeout(() => {
      set((state) => ({ toasts: state.toasts.filter((item) => item.id !== id) }))
    }, TOAST_TTL_MS)
  },

  dismiss: (id) => set((state) => ({ toasts: state.toasts.filter((item) => item.id !== id) })),
}))

/** 在非组件代码里发 toast。 */
export function toast(kind: ToastKind, text: string): void {
  useToastStore.getState().push(kind, text)
}
