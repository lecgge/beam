#!/usr/bin/env node
// 生成 Beam 桌面端所需的最小可用图标(§2 apps/desktop/src-tauri/icons/)。
// 纯 Node 实现,不引入图形库依赖:自写 PNG 编码器 + ICO/ICNS 容器打包。
// 用法: node scripts/gen-icons.mjs   (或 npm run icons)
import { deflateSync } from 'node:zlib'
import { writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const outDir = join(here, '..', 'src-tauri', 'icons')
mkdirSync(outDir, { recursive: true })

const crcTable = (() => {
  const t = new Int32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    t[n] = c
  }
  return t
})()

function crc32(buf) {
  let c = 0xffffffff
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length, 0)
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body), 0)
  return Buffer.concat([len, body, crc])
}

/** 编码 8 位 RGBA 像素为 PNG。 */
function encodePng(width, height, rgba) {
  const stride = width * 4
  const raw = Buffer.alloc((stride + 1) * height)
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0 // filter: None
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride)
  }
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(width, 0)
  ihdr.writeUInt32BE(height, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // color type RGBA
  ihdr[10] = 0
  ihdr[11] = 0
  ihdr[12] = 0
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

/** 画一个圆角方块 + 中央菱形,模拟原型的 Beam 菱形标志。 */
function drawIcon(size) {
  const rgba = Buffer.alloc(size * size * 4)
  const r = size * 0.22
  const c = size / 2
  const half = size * 0.24
  const put = (x, y, rr, gg, bb, aa) => {
    const i = (y * size + x) * 4
    rgba[i] = rr
    rgba[i + 1] = gg
    rgba[i + 2] = bb
    rgba[i + 3] = aa
  }
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      // 圆角矩形遮罩
      const dx = Math.max(r - x, 0, x - (size - 1 - r))
      const dy = Math.max(r - y, 0, y - (size - 1 - r))
      const inside = dx * dx + dy * dy <= r * r
      if (!inside) {
        put(x, y, 0, 0, 0, 0)
        continue
      }
      // 背景:蓝紫渐变
      const t = (x + y) / (2 * size)
      const bgR = Math.round(0x1f + (0x3d - 0x1f) * t)
      const bgG = Math.round(0x2a + (0x3f - 0x2a) * t)
      const bgB = Math.round(0x5c + (0xc9 - 0x5c) * t)
      put(x, y, bgR, bgG, bgB, 255)
    }
  }
  // 中央菱形(白色)
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const ax = Math.abs(x - c) / half
      const ay = Math.abs(y - c) / half
      if (ax + ay <= 1) put(x, y, 255, 255, 255, 255)
    }
  }
  return rgba
}

const pngSizes = {
  'icon.png': 512,
  '128x128@2x.png': 256,
  '128x128.png': 128,
  '32x32.png': 32,
  'icon-16.png': 16,
  'Square30x30Logo.png': 30,
  'Square44x44Logo.png': 44,
  'Square71x71Logo.png': 71,
  'Square89x89Logo.png': 89,
  'Square107x107Logo.png': 107,
  'Square142x142Logo.png': 142,
  'Square150x150Logo.png': 150,
  'Square284x284Logo.png': 284,
  'Square310x310Logo.png': 310,
  'StoreLogo.png': 50,
}

const pngBySize = new Map()
for (const [name, size] of Object.entries(pngSizes)) {
  const png = encodePng(size, size, drawIcon(size))
  pngBySize.set(size, png)
  writeFileSync(join(outDir, name), png)
  console.log(`wrote icons/${name} (${size}x${size}, ${png.length} B)`)
}

/** ICO:头部 + 目录项 + PNG 负载(Windows Vista+ 支持 PNG 压缩条目)。 */
function buildIco(entries) {
  const header = Buffer.alloc(6)
  header.writeUInt16LE(0, 0) // reserved
  header.writeUInt16LE(1, 2) // type = icon
  header.writeUInt16LE(entries.length, 4)
  const dir = Buffer.alloc(16 * entries.length)
  let offset = header.length + dir.length
  entries.forEach((e, i) => {
    const base = i * 16
    dir[base] = e.size >= 256 ? 0 : e.size
    dir[base + 1] = e.size >= 256 ? 0 : e.size
    dir[base + 2] = 0 // 调色板
    dir[base + 3] = 0
    dir.writeUInt16LE(1, base + 4) // color planes
    dir.writeUInt16LE(32, base + 6) // bpp
    dir.writeUInt32LE(e.png.length, base + 8)
    dir.writeUInt32LE(offset, base + 12)
    offset += e.png.length
  })
  return Buffer.concat([header, dir, ...entries.map((e) => e.png)])
}

const ico = buildIco([
  { size: 16, png: pngBySize.get(16) },
  { size: 32, png: pngBySize.get(32) },
  { size: 48, png: encodePng(48, 48, drawIcon(48)) },
  { size: 256, png: pngBySize.get(256) },
])
writeFileSync(join(outDir, 'icon.ico'), ico)
console.log(`wrote icons/icon.ico (${ico.length} B)`)

/** ICNS:PNG 负载条目(ic07/ic08/ic09/ic11/ic12/ic13/ic14)。 */
function icnsEntry(type, png) {
  const head = Buffer.alloc(8)
  head.write(type, 0, 4, 'ascii')
  head.writeUInt32BE(png.length + 8, 4)
  return Buffer.concat([head, png])
}

const icnsEntries = [
  icnsEntry('ic11', pngBySize.get(32)), // 32x32
  icnsEntry('ic12', pngBySize.get(64) ?? encodePng(64, 64, drawIcon(64))), // 64x64
  icnsEntry('ic13', pngBySize.get(256)), // 256x256
  icnsEntry('ic14', pngBySize.get(512)), // 512x512
]
const icnsHead = Buffer.alloc(8)
icnsHead.write('icns', 0, 4, 'ascii')
icnsHead.writeUInt32BE(8 + icnsEntries.reduce((n, e) => n + e.length, 0), 4)
const icns = Buffer.concat([icnsHead, ...icnsEntries])
writeFileSync(join(outDir, 'icon.icns'), icns)
console.log(`wrote icons/icon.icns (${icns.length} B)`)
