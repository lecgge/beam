import { useEffect } from 'react'

import { BackendNotice } from '../components/BackendNotice'
import { Sparkline, StatTile } from '../components/Charts'
import { Card, CardTitle, PageHeader } from '../components/Card'
import { EmptyState } from '../components/Feedback'
import { ErrorBanner } from '../components/ToastHost'
import { errorText } from '../ipc/errors'
import { useI18n } from '../i18n'
import { useHostStore, usePerfStore, useToastStore } from '../stores'

/**
 * 诊断页(§3.13 perf-stats,仅 Debug 构建)。
 *
 * 状态来源:`perfStore`——`perf://tick` 事件(250 ms)+ `perf_snapshot` / `perf_export` 命令。
 * Release 构建下后端返回 E11003,页面显示「仅 Debug 构建可用」,而不是报错崩溃。
 */
export function PerfPage() {
  const { t, language } = useI18n()
  const tick = usePerfStore((state) => state.tick)
  const snapshot = usePerfStore((state) => state.snapshot)
  const available = usePerfStore((state) => state.available)
  const exporting = usePerfStore((state) => state.exporting)
  const error = usePerfStore((state) => state.error)
  const load = usePerfStore((state) => state.load)
  const exportData = usePerfStore((state) => state.exportData)

  const backend = useHostStore((state) => state.status?.backend)
  const pushToast = useToastStore((state) => state.push)

  useEffect(() => {
    void load()
  }, [load])

  const handleExport = async (format: 'csv' | 'json') => {
    const result = await exportData(format)
    if (result) {
      pushToast('success', t('perf.exported', { path: result.path }))
    } else {
      pushToast('error', errorText(usePerfStore.getState().error, language))
    }
  }

  return (
    <div className="page">
      <PageHeader
        title={t('perf.title')}
        subtitle={t('perf.debugOnly')}
        extra={<span className="badge warn">{t('perf.debugOnly')}</span>}
      />

      {error ? <ErrorBanner code={error.code} message={errorText(error, language)} /> : null}
      {backend ? <BackendNotice backend={backend} /> : null}

      {!available && !snapshot ? (
        <Card>
          <EmptyState
            icon="🐞"
            title={t('perf.unavailable')}
            action={
              <button type="button" className="btn primary small" onClick={() => void load()}>
                {t('perf.snapshot')}
              </button>
            }
          />
        </Card>
      ) : null}

      {snapshot ? (
        <Card>
          <CardTitle extra={<span className="sub">{t('perf.samples', { count: snapshot.samples })}</span>}>
            {t('perf.e2eSeries')}
          </CardTitle>
          <Sparkline values={snapshot.e2eSeries} height={90} dashedAt={16.7} />
        </Card>
      ) : null}

      {tick ? (
        <Card>
          <CardTitle extra={<span className="badge ok">{t('perf.live')}</span>}>
            {t('perf.title')}
          </CardTitle>
          <div className="stat-grid">
            <StatTile label={t('connect.e2e')} value={tick.e2eMs.toFixed(1)} unit="ms" />
            <StatTile label={t('connect.fps')} value={String(tick.fps)} unit="fps" />
            <StatTile label={t('connect.bitrate')} value={tick.bitrateMbps.toFixed(1)} unit="Mbps" />
            <StatTile label="CPU" value={tick.cpuPct.toFixed(0)} unit="%" />
            <StatTile label="GPU" value={tick.gpuPct.toFixed(0)} unit="%" />
            <StatTile label={t('perf.memory')} value={String(tick.memoryMb)} unit="MB" />
          </div>
        </Card>
      ) : null}

      {snapshot ? (
        <Card>
          <CardTitle>{t('perf.stage')}</CardTitle>
          <div className="stage-list">
            {snapshot.stages.map((stage) => (
              <div key={stage.stage} className="stage-row">
                <span className="stage-name">{stage.stage}</span>
                <span className="sub">
                  {t('perf.mean')} {stage.meanMs.toFixed(1)} ms · {t('perf.p99')} {stage.p99Ms.toFixed(1)} ms
                </span>
              </div>
            ))}
          </div>
          <div className="stat-grid">
            {snapshot.metrics.map((metric) => (
              <StatTile key={metric.name} label={metric.name} value={metric.value} unit={metric.unit} />
            ))}
          </div>
          <div className="row">
            <button
              type="button"
              className="btn ghost"
              disabled={exporting}
              onClick={() => void handleExport('csv')}
            >
              {t('perf.exportCsv')}
            </button>
            <button
              type="button"
              className="btn ghost"
              disabled={exporting}
              onClick={() => void handleExport('json')}
            >
              {t('perf.exportJson')}
            </button>
          </div>
        </Card>
      ) : null}
    </div>
  )
}
