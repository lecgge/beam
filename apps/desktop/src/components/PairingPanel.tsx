import { useEffect, useState } from 'react'

import type { PairingCode } from '../ipc/types'
import { useI18n } from '../i18n'
import { QrCode } from './QrCode'
import { Spinner } from './Feedback'

/**
 * 添加新设备面板:二维码 + PIN。
 *
 * PIN 由 §3.3 定义为 4 位、120 秒有效;到期后后端轮换并推送 `pair://code-rotated`,
 * 这里按剩余时间倒数展示。
 */
export function PairingPanel({
  pairing,
  waiting,
}: {
  pairing: PairingCode | null
  waiting: boolean
}) {
  const { t } = useI18n()
  const [remaining, setRemaining] = useState(0)

  useEffect(() => {
    if (!pairing) return
    const tick = () => {
      const elapsed = Math.floor((Date.now() - pairing.rotatedAtMs) / 1000)
      setRemaining(Math.max(0, pairing.expiresInS - elapsed))
    }
    tick()
    const timer = window.setInterval(tick, 1000)
    return () => window.clearInterval(timer)
  }, [pairing])

  if (!pairing) {
    return (
      <div className="pairing-panel">
        <Spinner label={t('common.loading')} />
      </div>
    )
  }

  return (
    <div className="pairing-panel">
      <div className="pairing-title">{t('home.pairTitle')}</div>
      <p className="sub pairing-hint">{t('home.pairHint')}</p>
      <div className="pairing-qr">
        <QrCode text={pairing.qrPayload} />
      </div>
      <div className="pairing-pin" aria-label={t('home.pairTitle')}>
        {pairing.code.split('').map((digit, index) => (
          <span key={`${digit}-${index}`} className="pin-digit">
            {digit}
          </span>
        ))}
      </div>
      <p className="sub">
        {waiting ? (
          <Spinner label={t('home.waiting')} />
        ) : (
          t('home.pinExpires', { seconds: remaining })
        )}
      </p>
    </div>
  )
}
