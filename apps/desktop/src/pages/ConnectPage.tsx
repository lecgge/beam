import { useEffect, useState } from 'react'

import { BackendNotice } from '../components/BackendNotice'
import { Card, CardTitle, PageHeader } from '../components/Card'
import { HostRow } from '../components/DeviceRows'
import { EmptyState, Spinner } from '../components/Feedback'
import { LatencyOverlay } from '../components/LatencyOverlay'
import { Modal } from '../components/Modal'
import { PinInput } from '../components/PinInput'
import { ErrorBanner } from '../components/ToastHost'
import { errorText } from '../ipc/errors'
import type { DiscoveredHost } from '../ipc/types'
import { useI18n } from '../i18n'
import {
  useDeviceStore,
  useHostStore,
  usePairStore,
  useSessionStore,
  useSettingsStore,
  useToastStore,
} from '../stores'

/** §3.3 `pin_len=4`。 */
const PIN_LENGTH = 4

/**
 * 连接页(Windows 客户端侧:发现主机 → 连接 → 串流预览 + 统计)。
 *
 * 状态来源:`deviceStore.hosts`(`list_hosts` + `discovery://updated` 事件)、
 * `sessionStore`(`connect` / `disconnect` 命令 + `session://stats` 500 ms 事件)。
 *
 * 三个 §16 相关的点:
 * - 统计浮层受设置项 `showStatsOverlay` 控制(UX-05);
 * - 断线时盖遮罩,并按 `keepLastFrame` 决定是否保留最后一帧(UX-38);
 * - 未配对主机走 PIN 配对弹窗,错误态是「描红 + 剩余次数」(UX-34 / UX-37)。
 */
export function ConnectPage() {
  const { t, language } = useI18n()
  const hosts = useDeviceStore((state) => state.hosts)
  const loadHosts = useDeviceStore((state) => state.loadHosts)

  const session = useSessionStore((state) => state.session)
  const stats = useSessionStore((state) => state.stats)
  const reconnecting = useSessionStore((state) => state.reconnecting)
  const connecting = useSessionStore((state) => state.connecting)
  const error = useSessionStore((state) => state.error)
  const connect = useSessionStore((state) => state.connect)
  const disconnect = useSessionStore((state) => state.disconnect)

  // §16:串流浮层由设置项开关,关掉就不渲染。
  const showOverlay = useSettingsStore((state) => state.settings?.showStatsOverlay ?? true)

  const attemptsLeft = usePairStore((state) => state.attemptsLeft)
  const locked = usePairStore((state) => state.locked)
  const failAttempt = usePairStore((state) => state.fail)
  const resetAttempts = usePairStore((state) => state.reset)

  const backend = useHostStore((state) => state.status?.backend)
  const pushToast = useToastStore((state) => state.push)

  const [series, setSeries] = useState<number[]>([])
  const [pairTarget, setPairTarget] = useState<DiscoveredHost | null>(null)
  const [pin, setPin] = useState('')
  const [pinError, setPinError] = useState<{ code: string; message: string } | null>(null)

  useEffect(() => {
    void loadHosts()
  }, [loadHosts])

  // 端到端延迟曲线:由 `session://stats`(500 ms)累加出来。
  useEffect(() => {
    if (!stats) return
    setSeries((current) => [...current, stats.e2eMs].slice(-40))
  }, [stats])

  const handleConnect = async (hostId: string, name: string) => {
    const ok = await connect(hostId)
    pushToast(
      ok ? 'success' : 'error',
      ok ? t('connect.connected', { name }) : errorText(useSessionStore.getState().error, language),
    )
  }

  const handleDisconnect = async () => {
    const ok = await disconnect()
    setSeries([])
    pushToast(
      ok ? 'info' : 'error',
      ok ? t('connect.disconnected') : errorText(useSessionStore.getState().error, language),
    )
  }

  /** 打开 PIN 配对弹窗(未配对主机)。 */
  const handleOpenPair = (host: DiscoveredHost) => {
    setPairTarget(host)
    setPin('')
    setPinError(null)
    resetAttempts()
  }

  /**
   * 提交 PIN。
   *
   * 格式校验在前端做(§3.3 `pin_len=4`);失败算一次尝试,
   * 剩余次数按 §3.3 `max_attempts=3` 递减,归零即锁定。
   *
   * 注意:后端的 `connect` command 目前没有 PIN 参数(Rust 侧不在本目录范围),
   * 所以 PIN 的**最终校验**仍在后端——后端返回 E3001 / E3002 时,
   * 同样消耗这里的前端预算并展示「剩余 N 次」。
   */
  const handleSubmitPin = async () => {
    const target = pairTarget
    if (!target || locked) return

    if (!/^\d+$/.test(pin) || pin.length !== PIN_LENGTH) {
      failAttempt()
      setPinError({ code: 'E3001', message: t('connect.pinInvalid') })
      return
    }

    const ok = await connect(target.id)
    if (ok) {
      resetAttempts()
      setPairTarget(null)
      setPinError(null)
      pushToast('success', t('connect.connected', { name: target.name }))
      return
    }

    const failure = useSessionStore.getState().error
    if (failure?.code === 'E3001' || failure?.code === 'E3002') {
      failAttempt()
      setPinError({ code: failure.code, message: errorText(failure, language) })
      return
    }

    // 其它错误(网络 / 会话 / 主机不存在等)交给页面级错误条,关闭弹窗。
    setPairTarget(null)
  }

  // 断线遮罩:重连时盖住画面;`keepLastFrame` 为真时保留最后一帧。
  const keepLastFrame = reconnecting?.keepLastFrame ?? false
  const showFrame = !reconnecting || keepLastFrame
  const pinFailure = error?.code === 'E3001' || error?.code === 'E3002'

  return (
    <div className="page">
      <PageHeader title={t('connect.title')} subtitle={t('connect.subtitle')} />

      {error ? (
        <ErrorBanner
          code={error.code}
          message={errorText(error, language)}
          attemptsLeft={pinFailure ? attemptsLeft : undefined}
        />
      ) : null}
      {backend ? <BackendNotice backend={backend} /> : null}

      {session ? (
        <Card>
          <CardTitle
            extra={
              <button type="button" className="btn ghost small" onClick={() => void handleDisconnect()}>
                {t('connect.disconnect')}
              </button>
            }
          >
            {session.hostName} · {session.resolution}@{session.fps} · {session.codec}
          </CardTitle>

          <div className="preview">
            <div className="preview-stage">
              {showFrame ? <div className="preview-frame">GAME</div> : null}
              {reconnecting ? (
                <div className="preview-overlay">
                  <Spinner label={t('connect.reconnecting')} />
                  {keepLastFrame ? <span className="sub">{t('connect.lastFrameKept')}</span> : null}
                </div>
              ) : null}
            </div>
            <div className="preview-side">
              {showOverlay ? (
                stats ? (
                  <LatencyOverlay stats={stats} series={series} />
                ) : (
                  <p className="sub">{t('common.loading')}</p>
                )
              ) : (
                <p className="sub">{t('connect.overlayOff')}</p>
              )}
            </div>
          </div>
          <p className="sub preview-hint">{t('connect.previewHint')}</p>
        </Card>
      ) : null}

      <Card>
        <CardTitle>{t('connect.title')}</CardTitle>
        {hosts.length === 0 ? (
          <EmptyState
            icon="🖥"
            title={t('connect.notFound')}
            hint={t('connect.searching')}
            action={
              <button type="button" className="btn primary small" onClick={() => void loadHosts()}>
                {t('common.rescan')}
              </button>
            }
          />
        ) : (
          hosts.map((host) => (
            <HostRow
              key={host.id}
              host={host}
              actions={
                host.trusted ? (
                  <button
                    type="button"
                    className="btn primary small"
                    disabled={connecting || !host.online}
                    onClick={() => void handleConnect(host.id, host.name)}
                  >
                    {connecting ? t('connect.connecting') : t('connect.connect')}
                  </button>
                ) : (
                  <button
                    type="button"
                    className="btn secondary small"
                    disabled={!host.online}
                    onClick={() => handleOpenPair(host)}
                  >
                    {t('connect.pairAction')}
                  </button>
                )
              }
            />
          ))
        )}
      </Card>

      <Modal
        open={pairTarget !== null}
        title={t('connect.pairTitle')}
        closeLabel={t('common.close')}
        onClose={() => setPairTarget(null)}
        actions={
          <>
            <button type="button" className="btn secondary" onClick={() => setPairTarget(null)}>
              {t('common.cancel')}
            </button>
            <button
              type="button"
              className="btn primary"
              disabled={locked || connecting || pin.length !== PIN_LENGTH}
              onClick={() => void handleSubmitPin()}
            >
              {t('connect.pairSubmit')}
            </button>
          </>
        }
      >
        <p className="sub">{t('connect.pairHint')}</p>
        <PinInput
          value={pin}
          onChange={setPin}
          disabled={locked}
          invalid={pinError !== null}
          ariaLabel={t('connect.pairTitle')}
        />
        {pinError ? (
          <ErrorBanner code={pinError.code} message={pinError.message} attemptsLeft={attemptsLeft} />
        ) : null}
        {locked ? <p className="sub">{t('connect.pinLocked')}</p> : null}
      </Modal>
    </div>
  )
}
