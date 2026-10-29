import type { SessionStats } from '../ipc/types'
import { useI18n } from '../i18n'
import { Sparkline } from './Charts'

/** 串流统计浮层(对应 docs/images/win-4-latency-overlay.png)。 */
export function LatencyOverlay({
  stats,
  series = [],
}: {
  stats: SessionStats
  series?: number[]
}) {
  const { t } = useI18n()

  const breakdown: { label: string; value: number }[] = [
    { label: t('connect.capture'), value: stats.captureMs },
    { label: t('connect.encode'), value: stats.encodeMs },
    { label: t('connect.network'), value: stats.networkMs },
    { label: t('connect.decode'), value: stats.decodeMs },
    { label: t('connect.render'), value: stats.renderMs },
  ]

  return (
    <div className="latency-overlay">
      <div className="overlay-head">
        <span>{t('connect.e2e')}</span>
        <strong className="overlay-e2e">{stats.e2eMs.toFixed(1)} ms</strong>
      </div>

      <div className="overlay-bar" aria-hidden="true">
        {breakdown.map((item) => (
          <span
            key={item.label}
            style={{ flexGrow: Math.max(item.value, 0.1) }}
            title={`${item.label} ${item.value.toFixed(1)} ms`}
          />
        ))}
      </div>

      <dl className="overlay-rows">
        {breakdown.map((item) => (
          <div key={item.label} className="overlay-row">
            <dt>
              <i className="swatch" />
              {item.label}
            </dt>
            <dd>{item.value.toFixed(1)} ms</dd>
          </div>
        ))}
      </dl>

      <dl className="overlay-rows secondary">
        <div className="overlay-row">
          <dt>{t('connect.resolution')}</dt>
          <dd>
            {stats.resolution} @ {stats.fps}
          </dd>
        </div>
        <div className="overlay-row">
          <dt>{t('connect.bitrate')}</dt>
          <dd>{stats.bitrateMbps.toFixed(1)} Mbps</dd>
        </div>
        <div className="overlay-row">
          <dt>{t('connect.loss')}</dt>
          <dd>
            {stats.lossPct.toFixed(1)}% / {stats.fecRecoveredPct.toFixed(0)}%
          </dd>
        </div>
        <div className="overlay-row">
          <dt>{t('connect.jitter')}</dt>
          <dd>{stats.jitterMs.toFixed(1)} ms</dd>
        </div>
      </dl>

      {series.length > 1 ? <Sparkline values={series} height={44} dashedAt={16.7} /> : null}
    </div>
  )
}
