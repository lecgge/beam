import { useEffect } from 'react'

import { BackendNotice } from '../components/BackendNotice'
import { Card, CardTitle, PageHeader } from '../components/Card'
import { DeviceRow } from '../components/DeviceRows'
import { EmptyState } from '../components/Feedback'
import { ErrorBanner } from '../components/ToastHost'
import { PairingPanel } from '../components/PairingPanel'
import { StatusPill } from '../components/StatusPill'
import { errorText } from '../ipc/errors'
import { useI18n } from '../i18n'
import {
  selectIsHosting,
  useDeviceStore,
  useHostStore,
  useSettingsStore,
  useToastStore,
} from '../stores'
import type { PageKey } from '../components/Sidebar'

/**
 * 主机页(§3.11 host-app 的主入口)。
 *
 * 状态来源:`hostStore`(get_host_status + host://state-changed)、
 * `deviceStore.devices`(list_devices)、`deviceStore.pairing`(get_pairing_code + pair://code-rotated)。
 */
export function HomePage({ onNavigate }: { onNavigate: (page: PageKey) => void }) {
  const { t, language } = useI18n()
  const status = useHostStore((state) => state.status)
  const error = useHostStore((state) => state.error)
  const busy = useHostStore((state) => state.busy)
  const start = useHostStore((state) => state.start)
  const stop = useHostStore((state) => state.stop)

  const devices = useDeviceStore((state) => state.devices)
  const pairing = useDeviceStore((state) => state.pairing)
  const pendingPairs = useDeviceStore((state) => state.pairRequests.length)
  const loadDevices = useDeviceStore((state) => state.loadDevices)
  const loadPairing = useDeviceStore((state) => state.loadPairing)

  const settings = useSettingsStore((state) => state.settings)
  const pushToast = useToastStore((state) => state.push)

  // 页面挂载时补一次拉取,保证从托盘恢复窗口后数据是最新的。
  useEffect(() => {
    void loadDevices()
    void loadPairing()
  }, [loadDevices, loadPairing])

  const hosting = selectIsHosting(status)
  const streaming = status?.state === 'streaming'

  const handleStart = async () => {
    const ok = await start({
      resolution: settings?.resolution,
      fps: settings?.fps,
      codec: settings?.codec,
      bitrateMbps: settings?.bitrateMbps,
      gamepad: true,
    })
    pushToast(
      ok ? 'success' : 'error',
      ok ? t('home.started') : errorText(useHostStore.getState().error, language),
    )
  }

  const handleStop = async () => {
    const ok = await stop()
    pushToast(ok ? 'success' : 'error', ok ? t('home.stopped') : errorText(useHostStore.getState().error, language))
  }

  const recent = devices.filter((device) => device.lastSession !== null)

  return (
    <div className="page">
      <PageHeader
        title={t('home.title')}
        subtitle={t('home.subtitle', {
          name: status?.deviceName ?? '—',
          ip: status?.localIp ?? t('home.ipUnknown'),
        })}
        extra={
          // §11:性能统计的隐藏入口仅 Debug 构建存在。
          import.meta.env.DEV ? (
            <button type="button" className="btn ghost small" onClick={() => onNavigate('perf')}>
              📊 {t('perf.openDebug')}
            </button>
          ) : null
        }
      />

      {error ? <ErrorBanner code={error.code} message={errorText(error, language)} /> : null}
      {status ? <BackendNotice backend={status.backend} /> : null}

      <div className="home-grid">
        <Card>
          <div className="row spread">
            <StatusPill tone={hosting ? 'ok' : 'muted'}>
              {streaming ? t('home.hosting') : hosting ? t('home.hosting') : t('home.standby')}
              {hosting && !streaming ? ` · ${t('home.noDevice')}` : ''}
            </StatusPill>
            <span className="chip">
              {status?.encoder ?? '—'} · {status?.codec ?? '—'}
            </span>
          </div>

          <div className="metric-row">
            <div className="metric">
              <div className="sub">{t('home.gpu')}</div>
              <b>{status?.gpu ?? t('common.unknown')}</b>
            </div>
            <div className="metric">
              <div className="sub">{t('home.quality')}</div>
              <b>
                {status?.resolution ?? '—'} · {status?.fps ?? '—'}fps
              </b>
            </div>
            <div className="metric">
              <div className="sub">{t('home.bitrate')}</div>
              <b>{status?.bitrateMbps ?? '—'} Mbps</b>
            </div>
          </div>

          {hosting ? (
            <button type="button" className="btn danger big" disabled={busy} onClick={handleStop}>
              ■ {t('home.stop')}
            </button>
          ) : (
            <button type="button" className="btn primary big" disabled={busy} onClick={handleStart}>
              ▶ {t('home.start')}
            </button>
          )}

          <div className="chip-row">
            <span className="chip">
              🖥 {t('home.virtualDisplay')}:{settings?.matchClientResolution ? t('home.auto') : t('home.off')}
            </span>
            <span className="chip">🎮 {t('home.gamepad')}</span>
            <span className="chip">🔊 {t('home.audio')}</span>
          </div>
        </Card>

        <Card>
          <PairingPanel pairing={pairing} waiting={pendingPairs > 0} />
        </Card>
      </div>

      <Card>
        <CardTitle>{t('home.recent')}</CardTitle>
        {recent.length === 0 ? (
          <EmptyState
            icon="📡"
            title={t('common.empty')}
            hint={t('devices.emptyHint')}
            action={
              <button type="button" className="btn primary small" onClick={() => onNavigate('devices')}>
                {t('home.addDevice')}
              </button>
            }
          />
        ) : (
          recent.map((device) => (
            <DeviceRow
              key={device.id}
              device={device}
              statusLabel={
                device.lastSession
                  ? `${device.lastSession.resolution}${device.lastSession.fps} · ${t(
                      'home.avgLatency',
                      { ms: device.lastSession.avgLatencyMs?.toFixed(0) ?? '—' },
                    )}`
                  : undefined
              }
            />
          ))
        )}
      </Card>
    </div>
  )
}
