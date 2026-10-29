import { useI18n } from '../i18n'

export type PageKey = 'home' | 'devices' | 'connect' | 'perf' | 'settings'

const NAV_ITEMS: { key: PageKey; icon: string; labelKey: string }[] = [
  { key: 'home', icon: '🏠', labelKey: 'nav.home' },
  { key: 'devices', icon: '🔗', labelKey: 'nav.devices' },
  { key: 'connect', icon: '🖥', labelKey: 'nav.connect' },
  { key: 'perf', icon: '📊', labelKey: 'nav.perf' },
  { key: 'settings', icon: '⚙️', labelKey: 'nav.settings' },
]

/** 左侧导航。 */
export function Sidebar({
  current,
  onNavigate,
}: {
  current: PageKey
  onNavigate: (page: PageKey) => void
}) {
  const { t } = useI18n()

  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-mark">◆</span>
        <span className="brand-name">{t('app.name')}</span>
      </div>
      <nav className="nav">
        {NAV_ITEMS.map((item) => (
          <button
            key={item.key}
            type="button"
            className={`nav-item ${item.key === current ? 'on' : ''}`}
            aria-current={item.key === current ? 'page' : undefined}
            onClick={() => onNavigate(item.key)}
          >
            <span className="nav-icon" aria-hidden="true">
              {item.icon}
            </span>
            <span>{t(item.labelKey)}</span>
          </button>
        ))}
      </nav>
    </aside>
  )
}
