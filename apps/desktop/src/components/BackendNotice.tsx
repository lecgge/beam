import type { ReactNode } from 'react'

/**
 * 后端来源提示。
 *
 * 后端是本地桩实现时,界面必须明确标注「演示数据」,
 * 避免把合成的设备列表与延迟统计误读成实测结果(§3.11 纪律:不编造实测数据)。
 */
export function BackendNotice({
  backend,
  children,
}: {
  backend: 'stub' | 'beamCrates'
  children?: ReactNode
}) {
  if (backend !== 'stub') return null

  return (
    <div className="backend-notice">
      <span className="badge warn">本地桩实现</span>
      <span className="sub">
        {children ??
          '当前后端是本地桩实现:设备列表与延迟统计为演示用合成数据,不是实测结果。等 crates/beam-host 等落地后会自动切换。'}
      </span>
    </div>
  )
}
