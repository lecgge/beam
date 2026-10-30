/**
 * i18n:中文优先,结构上预留 en 切换。
 *
 * - 默认语言 zh-CN(§2 要求中文优先)。
 * - `en` 词表与 zh-CN 结构完全一致(由 `typeof zhCN` 约束),
 *   缺少的键会在编译期报错,不会在运行时露出半吊子英文。
 * - 语言来源:设置项 `settings.language`(§3.14),由 App 在启动时读入。
 */

import { createContext, useContext, useMemo, type ReactNode } from 'react'

import { en } from './en'
import { zhCN, type Dictionary, type Language } from './zh-CN'

export type { Language }

export const LANGUAGES: Language[] = ['zh-CN', 'en']

const DICTIONARIES: Record<Language, Dictionary> = {
  'zh-CN': zhCN,
  en,
}

export interface I18nContextValue {
  language: Language
  setLanguage: (language: Language) => void
  /** 取文案;`key` 是点号路径,如 `home.start`。 */
  t: (key: string, params?: Record<string, string | number>) => string
}

const I18nContext = createContext<I18nContextValue | null>(null)

/** 按点号路径在词表里取值。 */
function lookup(dictionary: Dictionary, key: string): string | undefined {
  const path = key.split('.')
  let cursor: unknown = dictionary
  for (const segment of path) {
    if (cursor === null || typeof cursor !== 'object') return undefined
    cursor = (cursor as Record<string, unknown>)[segment]
  }
  return typeof cursor === 'string' ? cursor : undefined
}

/** 把 `{name}` 之类的占位符替换成实际参数。 */
function interpolate(template: string, params?: Record<string, string | number>): string {
  if (!params) return template
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = params[name]
    return value === undefined ? match : String(value)
  })
}

export interface I18nProviderProps {
  language: Language
  onLanguageChange?: (language: Language) => void
  children: ReactNode
}

export function I18nProvider({ language, onLanguageChange, children }: I18nProviderProps) {
  const value = useMemo<I18nContextValue>(
    () => ({
      language,
      setLanguage: (next: Language) => onLanguageChange?.(next),
      t: (key: string, params?: Record<string, string | number>) => {
        const dictionary = DICTIONARIES[language] ?? zhCN
        // 中文缺词时回退到中文表,英文缺词时回退到 key 本身,便于发现遗漏。
        const text = lookup(dictionary, key) ?? lookup(zhCN, key) ?? key
        return interpolate(text, params)
      },
    }),
    [language, onLanguageChange],
  )

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

/** 读取 i18n 上下文。 */
export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext)
  if (!context) {
    throw new Error('useI18n 必须在 <I18nProvider> 内部使用')
  }
  return context
}

export { zhCN, en }
export type { Dictionary }
