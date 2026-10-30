import { useI18n } from '../i18n'
import { useToastStore } from '../stores/toastStore'
import { errorText } from '../ipc/errors'

/** 全局 toast 容器(§16 组件清单里的「Toast」)。 */
export function ToastHost() {
  const toasts = useToastStore((state) => state.toasts)
  const dismiss = useToastStore((state) => state.dismiss)

  if (toasts.length === 0) return null

  return (
    <div className="toast-host">
      {toasts.map((item) => (
        <div key={item.id} className={`toast ${item.kind}`} onClick={() => dismiss(item.id)}>
          {item.text}
        </div>
      ))}
    </div>
  )
}

/**
 * 错误条:按错误码展示当前语言的文案(§3.11)。
 *
 * §16 的状态行要求错误态是「描红 + 剩余次数」,所以这里额外接受
 * `attemptsLeft`(例如 PIN 配对失败时的剩余尝试次数)。
 */
export function ErrorBanner({
  code,
  message,
  onRetry,
  attemptsLeft,
}: {
  code?: string | null
  message?: string | null
  onRetry?: () => void
  /** 剩余可试次数;给出时在描红边框旁显示一个红色胶囊。 */
  attemptsLeft?: number
}) {
  const { t, language } = useI18n()
  if (!code) return null

  return (
    <div className="error-banner" role="alert">
      <span className="error-code">{code}</span>
      <span className="error-text">
        {message || errorText({ code, message: message ?? '', retryable: false }, language)}
      </span>
      {typeof attemptsLeft === 'number' ? (
        <span className="error-attempts">{t('common.attemptsLeft', { count: attemptsLeft })}</span>
      ) : null}
      {onRetry ? (
        <button type="button" className="btn ghost small" onClick={onRetry}>
          {t('common.retry')}
        </button>
      ) : null}
    </div>
  )
}
