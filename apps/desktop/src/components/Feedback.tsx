/** 加载指示器。 */
export function Spinner({ label }: { label?: string }) {
  return (
    <span className="spinner-wrap">
      <i className="spinner" />
      {label ? <span className="sub">{label}</span> : null}
    </span>
  )
}

/** 空状态。 */
export function EmptyState({
  icon = '·',
  title,
  hint,
  action,
}: {
  icon?: string
  title: string
  hint?: string
  action?: React.ReactNode
}) {
  return (
    <div className="empty-state">
      <div className="empty-icon">{icon}</div>
      <p className="empty-title">{title}</p>
      {hint ? <p className="sub">{hint}</p> : null}
      {action ? <div className="empty-action">{action}</div> : null}
    </div>
  )
}
