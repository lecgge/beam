/** 指标小方块:性能统计页的帧率 / 码率 / CPU 等。 */
export function StatTile({
  label,
  value,
  unit,
}: {
  label: string
  value: string
  unit?: string
}) {
  return (
    <div className="stat-tile">
      <div className="sub">{label}</div>
      <div className="stat-value">
        {value}
        {unit ? <span className="stat-unit"> {unit}</span> : null}
      </div>
    </div>
  )
}

/**
 * 折线图(端到端延迟序列)。
 *
 * §3.13 的 E2E 曲线;没有数据时给一个占位框,避免页面塌陷。
 */
export function Sparkline({
  values,
  height = 72,
  dashedAt,
}: {
  values: number[]
  height?: number
  /** 参考虚线的 y 值(例如一帧 16.7 ms)。 */
  dashedAt?: number
}) {
  if (values.length < 2) {
    return (
      <div className="sparkline empty" style={{ height }}>
        <span className="sub">—</span>
      </div>
    )
  }

  const width = 300
  const min = Math.min(...values, dashedAt ?? Infinity)
  const max = Math.max(...values, dashedAt ?? -Infinity)
  const span = max - min || 1
  const pad = span * 0.15
  const low = min - pad
  const high = max + pad
  const range = high - low || 1

  const toY = (value: number) => height - ((value - low) / range) * height
  const points = values
    .map((value, index) => `${(index / (values.length - 1)) * width},${toY(value)}`)
    .join(' ')

  return (
    <svg
      className="sparkline"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      style={{ height }}
      role="img"
      aria-label="端到端延迟曲线"
    >
      {dashedAt !== undefined ? (
        <line
          x1="0"
          x2={width}
          y1={toY(dashedAt)}
          y2={toY(dashedAt)}
          className="sparkline-ref"
        />
      ) : null}
      <polyline points={points} className="sparkline-line" />
    </svg>
  )
}
