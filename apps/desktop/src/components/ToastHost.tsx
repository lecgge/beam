import { useI18n } from '../i18n'
import { useToastStore } from '../stores/toastStore'
import { errorText } from '../ipc/errors'

/** 全局 toast 容器。 */
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

/** 错误条:按错误码展示中文文案(§3.11)。 */
export function ErrorBanner({
  code,
  message,
  onRetry,
}: {
  code?: string | null
  message?: string | null
  onRetry?: () => void
}) {
  const { t } = useI18n()
  if (!code) return null

  return (
    <div className="error-banner" role="alert">
      <span className="error-code">{code}</span>
      <span className="error-text">{message || errorText({ code, message: message ?? '', retryable: false })}</span>
      {onRetry ? (
        <button type="button" className="btn ghost small" onClick={onRetry}>
          {t('common.retry')}
        </button>
      ) : null}
    </div>
  )
}
