import { useEffect, useState } from 'react'

import { BackendNotice } from '../components/BackendNotice'
import { Card, CardTitle, PageHeader } from '../components/Card'
import { DeviceRow, HostRow, PairRequestRow } from '../components/DeviceRows'
import { EmptyState } from '../components/Feedback'
import { Modal } from '../components/Modal'
import { ErrorBanner } from '../components/ToastHost'
import { errorText } from '../ipc/errors'
import type { Device } from '../ipc/types'
import { useI18n } from '../i18n'
import { useDeviceStore, useHostStore, useToastStore } from '../stores'
import type { PageKey } from '../components/Sidebar'

/**
 * 设备页(§3.3 信任库 + §3.4 发现)。
 *
 * 状态来源:`deviceStore`——`list_devices` / `list_pair_requests` / `list_hosts` 拉取,
 * `pair://request` 与 `discovery://updated` 事件增量更新。
 *
 * 删除已配对设备是破坏性操作,走 §16 要求的模态弹窗二次确认(UX-31),
 * 不再用浏览器原生 `window.confirm`。
 */
export function DevicesPage({ onNavigate }: { onNavigate: (page: PageKey) => void }) {
  const { t, language } = useI18n()
  const devices = useDeviceStore((state) => state.devices)
  const pairRequests = useDeviceStore((state) => state.pairRequests)
  const hosts = useDeviceStore((state) => state.hosts)
  const busy = useDeviceStore((state) => state.busy)
  const error = useDeviceStore((state) => state.error)
  const loadAll = useDeviceStore((state) => state.loadAll)
  const loadHosts = useDeviceStore((state) => state.loadHosts)
  const approve = useDeviceStore((state) => state.approve)
  const reject = useDeviceStore((state) => state.reject)
  const remove = useDeviceStore((state) => state.remove)

  const backend = useHostStore((state) => state.status?.backend)
  const pushToast = useToastStore((state) => state.push)

  const [removeTarget, setRemoveTarget] = useState<Device | null>(null)

  useEffect(() => {
    void loadAll()
  }, [loadAll])

  const handleApprove = async (id: string, name: string) => {
    const device = await approve(id)
    pushToast(
      device ? 'success' : 'error',
      device
        ? t('devices.allowed', { name })
        : errorText(useDeviceStore.getState().error, language),
    )
  }

  const handleReject = async (id: string, name: string) => {
    const ok = await reject(id)
    pushToast(
      ok ? 'info' : 'error',
      ok ? t('devices.denied', { name }) : errorText(useDeviceStore.getState().error, language),
    )
  }

  const handleRemove = async (device: Device) => {
    const ok = await remove(device.id)
    setRemoveTarget(null)
    pushToast(
      ok ? 'success' : 'error',
      ok
        ? t('devices.removed', { name: device.name })
        : errorText(useDeviceStore.getState().error, language),
    )
  }

  return (
    <div className="page">
      <PageHeader title={t('devices.title')} subtitle={t('devices.subtitle', { count: devices.length })} />

      {error ? <ErrorBanner code={error.code} message={errorText(error, language)} /> : null}
      {backend ? <BackendNotice backend={backend} /> : null}

      <Card>
        <CardTitle>{t('devices.title')}</CardTitle>
        {devices.length === 0 ? (
          <EmptyState
            icon="🔗"
            title={t('devices.empty')}
            hint={t('devices.emptyHint')}
            action={
              // 配对入口(二维码 + PIN)在主机页,空态的主操作直接带过去。
              <button type="button" className="btn primary small" onClick={() => onNavigate('home')}>
                {t('devices.pairNew')}
              </button>
            }
          />
        ) : (
          devices.map((device) => (
            <DeviceRow
              key={device.id}
              device={device}
              statusLabel={device.streaming ? t('devices.streaming') : device.online ? t('devices.online') : t('devices.offline')}
              actions={
                <>
                  <button type="button" className="btn ghost small">
                    {t('devices.permissions')}
                  </button>
                  {device.streaming ? (
                    <button type="button" className="btn ghost small">
                      {t('devices.disconnect')}
                    </button>
                  ) : device.online ? (
                    <button type="button" className="btn ghost small">
                      {t('devices.wake')}
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="btn ghost small danger"
                    disabled={busy}
                    onClick={() => setRemoveTarget(device)}
                  >
                    {t('common.remove')}
                  </button>
                </>
              }
            />
          ))
        )}
      </Card>

      {pairRequests.length > 0 ? (
        <Card>
          <CardTitle>{t('devices.discovery')}</CardTitle>
          {pairRequests.map((request) => (
            <PairRequestRow
              key={request.id}
              request={request}
              actions={
                <>
                  <button
                    type="button"
                    className="btn ghost small"
                    disabled={busy}
                    onClick={() => void handleReject(request.id, request.name)}
                  >
                    {t('devices.deny')}
                  </button>
                  <button
                    type="button"
                    className="btn primary small"
                    disabled={busy}
                    onClick={() => void handleApprove(request.id, request.name)}
                  >
                    {t('devices.allow')}
                  </button>
                </>
              }
            />
          ))}
        </Card>
      ) : null}

      <Card>
        <CardTitle>{t('devices.discovery')}</CardTitle>
        {hosts.length === 0 ? (
          <EmptyState
            icon="🖥"
            title={t('connect.notFound')}
            hint={t('connect.searching')}
            action={
              <button type="button" className="btn primary small" onClick={() => void loadHosts()}>
                {t('common.rescan')}
              </button>
            }
          />
        ) : (
          hosts.map((host) => (
            <HostRow key={host.id} host={host} />
          ))
        )}
      </Card>

      <Modal
        open={removeTarget !== null}
        title={t('devices.removeTitle')}
        closeLabel={t('common.close')}
        onClose={() => setRemoveTarget(null)}
        actions={
          <>
            <button type="button" className="btn secondary" onClick={() => setRemoveTarget(null)}>
              {t('common.cancel')}
            </button>
            <button
              type="button"
              className="btn danger"
              disabled={busy}
              onClick={() => {
                if (removeTarget) void handleRemove(removeTarget)
              }}
            >
              {t('devices.removeAction')}
            </button>
          </>
        }
      >
        <p>{removeTarget ? t('devices.removeConfirm', { name: removeTarget.name }) : ''}</p>
      </Modal>
    </div>
  )
}
