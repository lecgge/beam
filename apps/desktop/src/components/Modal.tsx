import { useEffect, useRef, type ReactNode } from 'react'

export interface ModalProps {
  /** 是否打开。 */
  open: boolean
  /** 标题行内容。 */
  title: ReactNode
  /** 关闭回调(ESC / 遮罩点击 / 关闭按钮都走它)。 */
  onClose: () => void
  /** 面板主体。 */
  children?: ReactNode
  /** 底部操作区(通常是「取消 / 确认」两个按钮)。 */
  actions?: ReactNode
  /** 关闭按钮的无障碍标签。 */
  closeLabel?: string
}

/**
 * 通用模态弹窗(§16 组件清单里的「模态弹窗」)。
 *
 * 结构是「遮罩 + 面板」两层:
 * - 遮罩用串流浮层色 `#0a0e18` 85%(与画面上的浮层同一套令牌);
 * - 面板用面板色 + 卡片圆角 12。
 *
 * 交互:ESC 关闭、点遮罩关闭、关闭按钮关闭;打开时把焦点移进面板,
 * Tab 在面板内循环,关闭后把焦点还给打开它的元素。
 *
 * 注意:焦点相关的副作用**只依赖 `open`**。`onClose` 存在 ref 里,
 * 否则调用方每次渲染传新的箭头函数都会让副作用重跑,把用户正在输入的
 * 焦点抢走(例如 PIN 输入格会一直失焦)。
 */
export function Modal({ open, title, onClose, children, actions, closeLabel }: ModalProps) {
  const panelRef = useRef<HTMLDivElement | null>(null)
  const restoreRef = useRef<HTMLElement | null>(null)
  const onCloseRef = useRef(onClose)

  useEffect(() => {
    onCloseRef.current = onClose
  }, [onClose])

  useEffect(() => {
    if (!open) return undefined

    restoreRef.current = document.activeElement as HTMLElement | null
    // 有 `data-autofocus` 的元素优先(例如 PIN 输入格),否则聚焦面板本身。
    const panel = panelRef.current
    const preferred = panel?.querySelector<HTMLElement>('[data-autofocus]')
    ;(preferred ?? panel)?.focus()

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        onCloseRef.current()
        return
      }
      if (event.key !== 'Tab') return

      // 焦点循环:只在面板内的可聚焦元素之间转。
      if (!panel) return
      const focusables = Array.from(
        panel.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      )
      if (focusables.length === 0) return
      const first = focusables[0]
      const last = focusables[focusables.length - 1]
      if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      } else if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      }
    }

    document.addEventListener('keydown', onKeyDown, true)
    return () => {
      document.removeEventListener('keydown', onKeyDown, true)
      restoreRef.current?.focus()
    }
  }, [open])

  if (!open) return null

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        // 只有点在遮罩本身(而不是面板里)时才关闭。
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div className="modal" role="dialog" aria-modal="true" ref={panelRef} tabIndex={-1}>
        <header className="modal-header">
          <h3 className="modal-title">{title}</h3>
          <button type="button" className="modal-close" onClick={onClose} aria-label={closeLabel}>
            ✕
          </button>
        </header>
        <div className="modal-body">{children}</div>
        {actions ? <div className="modal-actions">{actions}</div> : null}
      </div>
    </div>
  )
}
