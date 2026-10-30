/**
 * 错误映射约定。
 *
 * §3.11:command 失败时 Rust 侧返回 `{code:"E2001", message, retryable}`,
 * 前端**按 code 显示文案**——`message` 只用于日志与调试细节。
 *
 * 文案表放在 i18n 词表里(`i18n/zh-CN.ts` / `i18n/en.ts` 的 `errors` 段):
 * 中英文两份由 `Dictionary` 类型在编译期约束键名完全一致,这里只负责「码 → 文案」。
 * 码位覆盖 crates 侧真实会透传上来的错误(见 `i18n/*.ts` 里 `errors` 段的注释),
 * 因为传输层会把 `beam-net` 等 crate 的 `code_str()` 直接当作 code 上报。
 *
 * Tauri 2 会把 command 的 `Err` 负载序列化成 JSON **字符串**再 reject,
 * 所以这里同时兼容「JSON 字符串」与「已经是对象」两种形态。
 */

import { en } from '../i18n/en'
import { zhCN, type Dictionary, type Language } from '../i18n/zh-CN'

export interface BeamError {
  /** 形如 `E2001` 的稳定错误码。 */
  code: string
  /** 后端给出的技术性说明。 */
  message: string
  /** 是否建议重试。 */
  retryable: boolean
}

/**
 * 错误文案的语言,与 i18n 的 {@link Language} 取值一致。
 *
 * 单独起名是为了让调用点读起来更明确(「给错误文案用的语言」),
 * 底层仍然是 i18n 的同一个联合类型,不会两处漂移。
 */
export type ErrorLanguage = Language

type ErrorTable = Dictionary['errors']
type CommonTable = Dictionary['common']

/** 码 → 文案。两套词表都来自 i18n,键名由 `Dictionary` 类型保证一致。 */
const ERROR_TEXT: Record<ErrorLanguage, ErrorTable> = {
  'zh-CN': zhCN.errors,
  en: en.errors,
}

/** 通用词条(兜底文案、可重试提示)。 */
const COMMON_TEXT: Record<ErrorLanguage, CommonTable> = {
  'zh-CN': zhCN.common,
  en: en.common,
}

/** 未登记错误码的兜底文案(按语言取)。 */
export function fallbackErrorText(language: ErrorLanguage = 'zh-CN'): string {
  return COMMON_TEXT[language]?.errorFallback ?? COMMON_TEXT['zh-CN'].errorFallback
}

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

/** 按错误码取文案;未登记的码退回兜底文案,并附上后端 message 便于排查。 */
export function errorText(error: BeamError | null | undefined, language: ErrorLanguage = 'zh-CN'): string {
  if (!error) return ''
  const table = ERROR_TEXT[language] ?? ERROR_TEXT['zh-CN']
  const text = table[error.code as keyof ErrorTable]
  if (text) return text
  const fallback = fallbackErrorText(language)
  return error.message ? `${fallback}(${error.message})` : fallback
}

/** 供 UI 展示的完整描述:文案 + 可重试提示。 */
export function errorDetail(
  error: BeamError | null | undefined,
  language: ErrorLanguage = 'zh-CN',
): string {
  if (!error) return ''
  const base = errorText(error, language)
  if (!error.retryable) return base
  const hint = (COMMON_TEXT[language] ?? COMMON_TEXT['zh-CN']).retryableHint
  return `${base}(${hint})`
}

/** 供调试面板使用的原始码。 */
export function errorCode(error: BeamError | null | undefined): string {
  return error?.code ?? ''
}
