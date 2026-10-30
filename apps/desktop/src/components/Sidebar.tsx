import { useI18n } from '../i18n'

export type PageKey = 'home' | 'devices' | 'connect' | 'perf' | 'settings'

const NAV_ITEMS: { key: PageKey; icon: string; labelKey: string }[] = [
  { key: 'home', icon: '🏠', labelKey: 'nav.home' },
  { key: 'devices', icon: '🔗', labelKey: 'nav.devices' },
  { key: 'connect', icon: '🖥', labelKey: 'nav.connect' },
  { key: 'perf', icon: '📊', labelKey: 'nav.perf' },
  { key: 'settings', icon: '⚙️', labelKey: 'nav.settings' },
]

/**
 * 只在 Debug 构建存在的入口(§11:「隐藏入口仅 debug 构建存在」)。
 *
 * 性能统计后端在 release 下由 `perf-stats` feature 整体剔除,放一个常驻入口
 * 只会让用户点进去撞到「仅 Debug 构建可用」,所以 release 直接不渲染。
 */
const DEBUG_ONLY_KEYS: PageKey[] = ['perf']

/** 左侧导航。 */
export function Sidebar({
  current,
  onNavigate,
}: {
  current: PageKey
  onNavigate: (page: PageKey) => void
}) {
  const { t } = useI18n()

  const items = NAV_ITEMS.filter(
    (item) => !DEBUG_ONLY_KEYS.includes(item.key) || import.meta.env.DEV,
  )

  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-mark">◆</span>
        <span className="brand-name">{t('app.name')}</span>
      </div>
      <nav className="nav">
        {items.map((item) => (
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
