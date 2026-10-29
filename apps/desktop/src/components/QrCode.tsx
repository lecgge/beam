import { useEffect, useState } from 'react'

import QRCode from 'qrcode'

interface QrShape {
  path: string
  size: number
}

/**
 * 配对二维码。
 *
 * 载荷格式见 §3.3:`beam://pair?h=ip:port&fp=..&t=token`,
 * 由 `get_pairing_code` 返回,扫码后 Android 端发起 SPAKE2 PIN 交换。
 */
export function QrCode({ text, size = 208 }: { text: string; size?: number }) {
  const [shape, setShape] = useState<QrShape | null>(null)

  useEffect(() => {
    let cancelled = false
    if (!text) {
      setShape(null)
      return () => {
        cancelled = true
      }
    }
    try {
      const qr = QRCode.create(text, { errorCorrectionLevel: 'M' })
      const matrix = qr.modules
      let d = ''
      for (let y = 0; y < matrix.size; y++) {
        for (let x = 0; x < matrix.size; x++) {
          if (matrix.data[y * matrix.size + x]) {
            d += `M${x} ${y}h1v1h-1z`
          }
        }
      }
      if (!cancelled) setShape({ path: d, size: matrix.size })
    } catch {
      if (!cancelled) setShape(null)
    }
    return () => {
      cancelled = true
    }
  }, [text])

  if (!shape) {
    return (
      <div className="qr-placeholder" style={{ width: size, height: size }}>
        <span className="sub">—</span>
      </div>
    )
  }

  return (
    <svg
      className="qr"
      width={size}
      height={size}
      viewBox={`0 0 ${shape.size} ${shape.size}`}
      shapeRendering="crispEdges"
      role="img"
      aria-label="配对二维码"
    >
      <rect x="0" y="0" width={shape.size} height={shape.size} fill="#ffffff" />
      <path d={shape.path} fill="#000000" />
    </svg>
  )
}
