import type { Device, DiscoveredHost, PairRequest } from '../ipc/types'

/** 按平台给一个设备图标。 */
export function deviceIcon(platform: string): string {
  const value = platform.toLowerCase()
  if (value.includes('tv')) return '📺'
  if (value.includes('windows')) return '💻'
  if (value.includes('android')) return '📱'
  if (value.includes('harmony')) return '📟'
  return '🖥'
}

/** 已配对设备的一行。 */
export function DeviceRow({
  device,
  statusLabel,
  actions,
}: {
  device: Device
  statusLabel?: string
  actions?: React.ReactNode
}) {
  return (
    <div className="device-row">
      <div className="device-icon">{deviceIcon(device.platform)}</div>
      <div className="device-main">
        <div className="device-title">
          <span className="device-name">{device.name}</span>
          {device.streaming ? <span className="badge ok">串流中</span> : null}
        </div>
        <div className="sub device-meta">
          {device.platform} · 指纹 {device.fingerprint} · 配对于 {device.pairedAt}
        </div>
      </div>
      {statusLabel ? <div className={`device-status ${device.online ? 'ok' : ''}`}>{statusLabel}</div> : null}
      {actions ? <div className="device-actions">{actions}</div> : null}
    </div>
  )
}

/** 局域网发现到的主机的一行。 */
export function HostRow({
  host,
  actions,
}: {
  host: DiscoveredHost
  actions?: React.ReactNode
}) {
  return (
    <div className="device-row">
      <div className="device-icon">🖥</div>
      <div className="device-main">
        <div className="device-title">
          <span className="device-name">{host.name}</span>
          <span className={`badge ${host.trusted ? 'ok' : 'muted'}`}>
            {host.trusted ? '已配对' : '未配对'}
          </span>
        </div>
        <div className="sub device-meta">
          {host.address}:{host.port}
          {host.gpu ? ` · ${host.gpu}` : ''} · v{host.version}
        </div>
      </div>
      {actions ? <div className="device-actions">{actions}</div> : null}
    </div>
  )
}

/** 待确认的配对请求。 */
export function PairRequestRow({
  request,
  actions,
}: {
  request: PairRequest
  actions?: React.ReactNode
}) {
  return (
    <div className="device-row">
      <div className="device-icon">{deviceIcon(request.platform)}</div>
      <div className="device-main">
        <div className="device-title">
          <span className="device-name">{request.name}</span>
          <span className="badge warn">请求配对</span>
        </div>
        <div className="sub device-meta">
          {request.platform} · {request.address} · 指纹 {request.fingerprint}
        </div>
      </div>
      {actions ? <div className="device-actions">{actions}</div> : null}
    </div>
  )
}
