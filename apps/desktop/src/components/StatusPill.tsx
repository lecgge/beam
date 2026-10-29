import type { ReactNode } from 'react'

export type PillTone = 'ok' | 'warn' | 'err' | 'info' | 'muted'

/** 状态胶囊:在线 / 离线 / 串流中 / 待确认等。 */
export function StatusPill({
  tone = 'muted',
  children,
}: {
  tone?: PillTone
  children: ReactNode
}) {
  return <span className={`pill ${tone}`}>{children}</span>
}

/** 小圆点 + 文本的状态标记。 */
export function DotStatus({
  tone = 'muted',
  children,
}: {
  tone?: PillTone
  children: ReactNode
}) {
  return (
    <span className={`dot-status ${tone}`}>
      <i className="dot" />
      {children}
    </span>
  )
}
