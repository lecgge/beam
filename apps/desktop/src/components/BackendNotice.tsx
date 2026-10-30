import type { ReactNode } from 'react'

import { useI18n } from '../i18n'

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
  const { t } = useI18n()

  if (backend !== 'stub') return null

  return (
    <div className="backend-notice">
      <span className="badge warn">{t('backend.stub')}</span>
      <span className="sub">{children ?? t('backend.stubHint')}</span>
    </div>
  )
}
