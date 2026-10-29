import { useEffect, useState } from 'react'

import { BackendNotice } from '../components/BackendNotice'
import { Card, CardTitle, PageHeader } from '../components/Card'
import { HostRow } from '../components/DeviceRows'
import { EmptyState, Spinner } from '../components/Feedback'
import { LatencyOverlay } from '../components/LatencyOverlay'
import { ErrorBanner } from '../components/ToastHost'
import { errorText } from '../ipc/errors'
import { useI18n } from '../i18n'
import { useDeviceStore, useHostStore, useSessionStore, useToastStore } from '../stores'

/**
 * 连接页(Windows 客户端侧:发现主机 → 连接 → 串流预览 + 统计)。
 *
 * 状态来源:`deviceStore.hosts`(`list_hosts` + `discovery://updated` 事件)、
 * `sessionStore`(`connect` / `disconnect` 命令 + `session://stats` 500 ms 事件)。
 */
export function ConnectPage() {
  const { t } = useI18n()
  const hosts = useDeviceStore((state) => state.hosts)
  const loadHosts = useDeviceStore((state) => state.loadHosts)

  const session = useSessionStore((state) => state.session)
  const stats = useSessionStore((state) => state.stats)
  const reconnecting = useSessionStore((state) => state.reconnecting)
  const connecting = useSessionStore((state) => state.connecting)
  const error = useSessionStore((state) => state.error)
  const connect = useSessionStore((state) => state.connect)
  const disconnect = useSessionStore((state) => state.disconnect)

  const backend = useHostStore((state) => state.status?.backend)
  const pushToast = useToastStore((state) => state.push)

  const [series, setSeries] = useState<number[]>([])

  useEffect(() => {
    void loadHosts()
  }, [loadHosts])

  // 端到端延迟曲线:由 `session://stats`(500 ms)累加出来。
  useEffect(() => {
    if (!stats) return
    setSeries((current) => [...current, stats.e2eMs].slice(-40))
  }, [stats])

  const handleConnect = async (hostId: string, name: string) => {
    const ok = await connect(hostId)
    pushToast(
      ok ? 'success' : 'error',
      ok ? t('connect.connected', { name }) : errorText(useSessionStore.getState().error),
    )
  }

  const handleDisconnect = async () => {
    const ok = await disconnect()
    setSeries([])
    pushToast(ok ? 'info' : 'error', ok ? t('connect.disconnected') : errorText(useSessionStore.getState().error))
  }

  return (
    <div className="page">
      <PageHeader title={t('connect.title')} subtitle={t('connect.subtitle')} />

      {error ? <ErrorBanner code={error.code} message={errorText(error)} /> : null}
      {backend ? <BackendNotice backend={backend} /> : null}

      {session ? (
        <Card>
          <CardTitle
            extra={
              <button type="button" className="btn ghost small" onClick={() => void handleDisconnect()}>
                {t('connect.disconnect')}
              </button>
            }
          >
            {session.hostName} · {session.resolution}@{session.fps} · {session.codec}
          </CardTitle>

          <div className="preview">
            <div className="preview-stage">
              <div className="preview-placeholder">GAME</div>
              {reconnecting ? (
                <div className="preview-overlay">
                  <Spinner label={t('connect.reconnecting')} />
                </div>
              ) : null}
            </div>
            {stats ? (
              <div className="preview-side">
                <LatencyOverlay stats={stats} series={series} />
              </div>
            ) : (
              <div className="preview-side">
                <p className="sub">{t('common.loading')}</p>
              </div>
            )}
          </div>
          <p className="sub preview-hint">{t('connect.previewHint')}</p>
        </Card>
      ) : null}

      <Card>
        <CardTitle>{t('connect.title')}</CardTitle>
        {hosts.length === 0 ? (
          <EmptyState icon="🖥" title={t('connect.notFound')} hint={t('connect.searching')} />
        ) : (
          hosts.map((host) => (
            <HostRow
              key={host.id}
              host={host}
              actions={
                <button
                  type="button"
                  className="btn primary small"
                  disabled={connecting || !host.online}
                  onClick={() => void handleConnect(host.id, host.name)}
                >
                  {connecting ? t('connect.connecting') : t('connect.connect')}
                </button>
              }
            />
          ))
        )}
      </Card>
    </div>
  )
}
