import { useEffect, useState } from 'react'

import { Sidebar, type PageKey } from './components/Sidebar'
import { ToastHost } from './components/ToastHost'
import {
  onDiscoveryUpdated,
  onHostStateChanged,
  onPairCodeRotated,
  onPairRequest,
  onPerfTick,
  onSessionDisconnected,
  onSessionReconnecting,
  onSessionStats,
} from './ipc/events'
import { I18nProvider, type Language } from './i18n'
import { ConnectPage } from './pages/ConnectPage'
import { DevicesPage } from './pages/DevicesPage'
import { HomePage } from './pages/HomePage'
import { PerfPage } from './pages/PerfPage'
import { SettingsPage } from './pages/SettingsPage'
import {
  useDeviceStore,
  useHostStore,
  usePerfStore,
  useSessionStore,
  useSettingsStore,
} from './stores'

/**
 * 应用外壳:导航 + 页面路由 + 全局事件订阅。
 *
 * §3.11 的 8 个事件在这里统一订阅,并派发到各个 store;
 * 页面本身只从 store 读状态,不直接碰 IPC。
 */
export default function App() {
  const settingsLanguage = useSettingsStore((state) => state.settings?.language)
  const [language, setLanguage] = useState<Language>('zh-CN')

  // 设置里的语言是语言来源(§3.14),优先级高于前端默认值。
  useEffect(() => {
    if (settingsLanguage === 'zh-CN' || settingsLanguage === 'en') {
      setLanguage(settingsLanguage)
    }
  }, [settingsLanguage])

  return (
    <I18nProvider language={language} onLanguageChange={setLanguage}>
      <AppShell />
    </I18nProvider>
  )
}

function AppShell() {
  const [page, setPage] = useState<PageKey>('home')

  const loadHostStatus = useHostStore((state) => state.load)
  const loadSettings = useSettingsStore((state) => state.load)
  const loadDevices = useDeviceStore((state) => state.loadAll)
  const theme = useSettingsStore((state) => state.settings?.theme ?? 'dark')

  // 主题跟随设置(§3.14 `theme`),system 时跟随系统偏好。
  useEffect(() => {
    const root = document.documentElement
    if (theme === 'system') {
      const media = window.matchMedia('(prefers-color-scheme: light)')
      const apply = () => root.setAttribute('data-theme', media.matches ? 'light' : 'dark')
      apply()
      media.addEventListener('change', apply)
      return () => media.removeEventListener('change', apply)
    }
    root.setAttribute('data-theme', theme === 'light' ? 'light' : 'dark')
    return undefined
  }, [theme])

  // 首次装配:拉取各 store 的初始数据。
  useEffect(() => {
    void loadHostStatus()
    void loadSettings()
    void loadDevices()
  }, [loadHostStatus, loadSettings, loadDevices])

  // §3.11 的 8 个事件 → 各 store。
  useEffect(() => {
    const subscriptions = [
      onHostStateChanged((status) => useHostStore.getState().applyStatus(status)),
      onPairRequest((request) => useDeviceStore.getState().addPairRequest(request)),
      onPairCodeRotated((pairing) => useDeviceStore.getState().applyPairing(pairing)),
      onDiscoveryUpdated((hosts) => useDeviceStore.getState().applyHosts(hosts)),
      onSessionStats((stats) => useSessionStore.getState().applyStats(stats)),
      onSessionReconnecting((info) => useSessionStore.getState().applyReconnecting(info)),
      onSessionDisconnected(() => useSessionStore.getState().clearSession()),
      onPerfTick((tick) => usePerfStore.getState().applyTick(tick)),
    ]
    return () => {
      subscriptions.forEach((pending) => {
        void pending.then((unlisten) => unlisten()).catch(() => undefined)
      })
    }
  }, [])

  return (
    <div className="app">
      <Sidebar current={page} onNavigate={setPage} />
      <main className="app-main">
        {page === 'home' ? <HomePage /> : null}
        {page === 'devices' ? <DevicesPage /> : null}
        {page === 'connect' ? <ConnectPage /> : null}
        {page === 'perf' ? <PerfPage /> : null}
        {page === 'settings' ? <SettingsPage /> : null}
      </main>
      <ToastHost />
    </div>
  )
}
