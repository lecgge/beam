/**
 * 错误映射约定。
 *
 * §3.11:command 失败时 Rust 侧返回 `{code:"E2001", message, retryable}`,
 * 前端**按 code 显示中文文案**——`message` 只用于日志与调试细节。
 *
 * Tauri 2 会把 command 的 `Err` 负载序列化成 JSON **字符串**再 reject,
 * 所以这里同时兼容「JSON 字符串」与「已经是对象」两种形态。
 */

export interface BeamError {
  /** 形如 `E2001` 的稳定错误码。 */
  code: string
  /** 后端给出的技术性说明。 */
  message: string
  /** 是否建议重试。 */
  retryable: boolean
}

/**
 * 错误码 → 中文文案。
 *
 * 码位沿用 §3.1–§3.15 的分段:
 * E1 proto / E2 net / E3 pair / E4 discovery / E5 capture / E6 encode /
 * E7 audio / E8 input / E9 session / E10 decode / E11 config / E12 外壳。
 */
const ERROR_TEXT: Record<string, string> = {
  // E2xxx net
  E2001: '连接超时,请检查网络后重试',
  E2002: '对方拒绝了证书校验,请确认对方是可信主机',
  E2003: '当前网络不支持无连接数据报,串流不可用',
  E2004: '部分视频帧已过期丢弃,画面可能有卡顿',
  // E3xxx pair
  E3001: 'PIN 码不正确,请核对主机屏幕上的数字',
  E3002: 'PIN 尝试次数过多,已锁定 60 秒',
  E3003: '配对二维码已过期,请刷新后重新扫码',
  E3004: '设备指纹与预期不符,可能是中间人攻击,已拒绝',
  // E4xxx discovery
  E4001: '局域网搜索失败,请检查防火墙与网卡设置',
  // E5xxx capture
  E5001: '没有找到可抓取的显示器',
  E5002: '独占全屏应用无法抓取,已尝试切换到 DXGI 模式',
  E5003: '显示设备已丢失,请重新选择显示器',
  // E6xxx encode
  E6001: '没有找到可用的硬件编码器',
  E6002: '同时串流的会话数已达上限',
  E6003: '当前参数不被编码器支持,已自动降档',
  // E9xxx session
  E9001: '当前状态不允许执行该操作',
  E9002: '已经有一条串流会话,请先断开',
  E9003: '重连失败,会话已结束',
  E9101: '找不到该设备',
  E9102: '找不到该主机,请刷新列表后重试',
  // E11xxx config
  E11001: '设置项取值不合法',
  E11002: '配置文件读写失败',
  E11003: '性能统计仅在 Debug 构建可用',
  // E12xxx 应用外壳
  E12001: '未知的命令',
  // 兜底
  E9999: '发生未知错误',
}

/** 未登记错误码的兜底文案。 */
export const FALLBACK_ERROR_TEXT = '操作失败,请稍后重试'

/** 把 Tauri reject 出来的原始值规整成 {@link BeamError}。 */
export function toBeamError(raw: unknown): BeamError {
  if (typeof raw === 'string') {
    try {
      const parsed: unknown = JSON.parse(raw)
      if (parsed && typeof parsed === 'object' && 'code' in parsed) {
        return normalize(parsed as Partial<BeamError>)
      }
    } catch {
      // 不是 JSON,按普通错误消息处理
    }
    return { code: 'E9999', message: raw, retryable: false }
  }
  if (raw && typeof raw === 'object' && 'code' in raw) {
    return normalize(raw as Partial<BeamError>)
  }
  return {
    code: 'E9999',
    message: raw instanceof Error ? raw.message : String(raw),
    retryable: false,
  }
}

function normalize(raw: Partial<BeamError>): BeamError {
  return {
    code: typeof raw.code === 'string' ? raw.code : 'E9999',
    message: typeof raw.message === 'string' ? raw.message : '',
    retryable: raw.retryable === true,
  }
}

/** 按错误码取中文文案;未登记的码退回兜底文案,并附上后端 message 便于排查。 */
export function errorText(error: BeamError | null | undefined): string {
  if (!error) return ''
  const text = ERROR_TEXT[error.code] ?? FALLBACK_ERROR_TEXT
  if (ERROR_TEXT[error.code]) return text
  return error.message ? `${text}(${error.message})` : text
}

/** 供 UI 展示的完整描述:中文文案 + 可重试提示。 */
export function errorDetail(error: BeamError | null | undefined): string {
  if (!error) return ''
  const base = errorText(error)
  return error.retryable ? `${base}(可重试)` : base
}

/** 供调试面板使用的原始码。 */
export function errorCode(error: BeamError | null | undefined): string {
  return error?.code ?? ''
}
