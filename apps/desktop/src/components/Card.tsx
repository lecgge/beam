import type { ReactNode } from 'react'

/** 通用卡片容器。 */
export function Card({
  children,
  className = '',
}: {
  children: ReactNode
  className?: string
}) {
  return <section className={`card ${className}`.trim()}>{children}</section>
}

/** 卡片标题。 */
export function CardTitle({
  children,
  extra,
}: {
  children: ReactNode
  extra?: ReactNode
}) {
  return (
    <header className="card-title">
      <h3>{children}</h3>
      {extra ? <div className="card-title-extra">{extra}</div> : null}
    </header>
  )
}

/** 页面标题 + 副标题。 */
export function PageHeader({
  title,
  subtitle,
  extra,
}: {
  title: string
  subtitle?: string
  extra?: ReactNode
}) {
  return (
    <div className="page-header">
      <div>
        <h2>{title}</h2>
        {subtitle ? <p className="sub">{subtitle}</p> : null}
      </div>
      {extra ? <div className="page-header-extra">{extra}</div> : null}
    </div>
  )
}

/** 「标题 —— 值」的一行设置项。 */
export function SettingRow({
  label,
  children,
}: {
  label: string
  children: ReactNode
}) {
  return (
    <div className="setting-row">
      <span className="setting-label">{label}</span>
      <div className="setting-control">{children}</div>
    </div>
  )
}
