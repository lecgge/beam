import { Card, CardTitle, PageHeader, SettingRow } from '../components/Card'
import { ErrorBanner } from '../components/ToastHost'
import { SegmentedControl } from '../components/SegmentedControl'
import { Toggle } from '../components/Toggle'
import { errorText } from '../ipc/errors'
import { useI18n, type Language } from '../i18n'
import { useSettingsStore, useToastStore } from '../stores'

const RESOLUTIONS = ['720p', '1080p', '1440p', '4k']
const FRAMERATES = [30, 60, 90, 120, 165, 200]
const CODECS = ['hevc', 'h264', 'av1']
const ENCODERS = ['auto', 'nvenc', 'amf', 'qsv']
const CAPTURE_BACKENDS = ['auto', 'wgc', 'dxgi']
const PRESETS = ['low_latency_p1', 'balanced', 'quality']
const AUDIO_MODES = ['host+stream', 'stream_only', 'off']
const THEMES = ['dark', 'light', 'system']
const FEC_RATIOS = [0.1, 0.2, 0.3]

/**
 * 设置页(§3.14)。
 *
 * 状态来源:`settingsStore`——`get_settings` 拉全量,`set_settings(patch)` 增量写回。
 * 每次改动即时下发;失败按错误码显示中文文案并回滚到后端返回的值。
 */
export function SettingsPage() {
  const { t, language, setLanguage } = useI18n()
  const settings = useSettingsStore((state) => state.settings)
  const error = useSettingsStore((state) => state.error)
  const patch = useSettingsStore((state) => state.patch)
  const pushToast = useToastStore((state) => state.push)

  const apply = async (next: Parameters<typeof patch>[0], successText?: string) => {
    const ok = await patch(next)
    if (ok && successText) pushToast('success', successText)
    if (!ok) {
      pushToast('error', errorText(useSettingsStore.getState().error, language))
    }
  }

  if (!settings) {
    return (
      <div className="page">
        <PageHeader title={t('settings.title')} subtitle={t('settings.subtitle')} />
        <Card>
          <p className="sub">{t('common.loading')}</p>
        </Card>
      </div>
    )
  }

  return (
    <div className="page">
      <PageHeader title={t('settings.title')} subtitle={t('settings.subtitle')} />

      {error ? <ErrorBanner code={error.code} message={errorText(error, language)} /> : null}

      <div className="settings-grid">
        <Card>
          <CardTitle>{t('settings.quality')}</CardTitle>
          <SettingRow label={t('settings.resolution')}>
            <SegmentedControl
              ariaLabel={t('settings.resolution')}
              options={RESOLUTIONS.map((value) => ({ value, label: value }))}
              value={settings.resolution}
              onChange={(value) => void apply({ resolution: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.fps')}>
            <SegmentedControl
              ariaLabel={t('settings.fps')}
              options={FRAMERATES.map((value) => ({ value, label: String(value) }))}
              value={settings.fps}
              onChange={(value) => void apply({ fps: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.bitrate')}>
            <span>
              {settings.bitrateMbps} Mbps{' '}
              <span className="sub">
                ({t('settings.bitrateAdaptive', {
                  min: settings.bitrateMinMbps,
                  max: settings.bitrateMaxMbps,
                })})
              </span>
            </span>
          </SettingRow>
          <SettingRow label={t('settings.hdr')}>
            <Toggle
              checked={settings.hdr}
              ariaLabel={t('settings.hdr')}
              onChange={(value) => void apply({ hdr: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.matchClient')}>
            <Toggle
              checked={settings.matchClientResolution}
              ariaLabel={t('settings.matchClient')}
              onChange={(value) => void apply({ matchClientResolution: value })}
            />
          </SettingRow>
        </Card>

        <Card>
          <CardTitle>{t('settings.encoding')}</CardTitle>
          <SettingRow label={t('settings.encoder')}>
            <select
              className="select"
              value={settings.encoder}
              onChange={(event) => void apply({ encoder: event.target.value })}
            >
              {ENCODERS.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </SettingRow>
          <SettingRow label={t('settings.codec')}>
            <SegmentedControl
              ariaLabel={t('settings.codec')}
              options={CODECS.map((value) => ({ value, label: value.toUpperCase() }))}
              value={settings.codec}
              onChange={(value) => void apply({ codec: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.preset')}>
            <select
              className="select"
              value={settings.preset}
              onChange={(event) => void apply({ preset: event.target.value })}
            >
              {PRESETS.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </SettingRow>
          <SettingRow label={t('settings.captureBackend')}>
            <select
              className="select"
              value={settings.captureBackend}
              onChange={(event) => void apply({ captureBackend: event.target.value })}
            >
              {CAPTURE_BACKENDS.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </SettingRow>
        </Card>

        <Card>
          <CardTitle>{t('settings.network')}</CardTitle>
          <SettingRow label={t('settings.fec')}>
            <SegmentedControl
              ariaLabel={t('settings.fec')}
              options={FEC_RATIOS.map((value) => ({ value, label: `${value * 100}%` }))}
              value={settings.fecRatio}
              onChange={(value) => void apply({ fecRatio: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.port')}>
            <span className="sub">
              {settings.port} / UDP
            </span>
          </SettingRow>
          <SettingRow label={t('settings.discovery')}>
            <Toggle
              checked={settings.discoveryEnabled}
              ariaLabel={t('settings.discovery')}
              onChange={(value) => void apply({ discoveryEnabled: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.allowTailscale')}>
            <Toggle
              checked={settings.allowTailscale}
              ariaLabel={t('settings.allowTailscale')}
              onChange={(value) => void apply({ allowTailscale: value })}
            />
          </SettingRow>
        </Card>

        <Card>
          <CardTitle>{t('settings.audio')}</CardTitle>
          <SettingRow label={t('settings.audioMode')}>
            <select
              className="select"
              value={settings.audioMode}
              onChange={(event) => void apply({ audioMode: event.target.value })}
            >
              {AUDIO_MODES.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </SettingRow>
          <SettingRow label={t('settings.audioBitrate')}>
            <span className="sub">{settings.audioBitrateKbps} kbps</span>
          </SettingRow>
        </Card>

        <Card>
          <CardTitle>{t('settings.interface')}</CardTitle>
          <SettingRow label={t('settings.language')}>
            <select
              className="select"
              value={language}
              onChange={(event) => {
                const next = event.target.value as Language
                setLanguage(next)
                void apply({ language: next }, t('settings.saved'))
              }}
            >
              <option value="zh-CN">简体中文</option>
              <option value="en">English</option>
            </select>
          </SettingRow>
          <SettingRow label={t('settings.theme')}>
            <SegmentedControl
              ariaLabel={t('settings.theme')}
              options={THEMES.map((value) => ({ value, label: value }))}
              value={settings.theme}
              onChange={(value) => void apply({ theme: value }, t('settings.saved'))}
            />
          </SettingRow>
          <SettingRow label={t('settings.showOverlay')}>
            <Toggle
              checked={settings.showStatsOverlay}
              ariaLabel={t('settings.showOverlay')}
              onChange={(value) => void apply({ showStatsOverlay: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.minimizeToTray')}>
            <Toggle
              checked={settings.minimizeToTray}
              ariaLabel={t('settings.minimizeToTray')}
              onChange={(value) => void apply({ minimizeToTray: value })}
            />
          </SettingRow>
          <SettingRow label={t('settings.launchOnStartup')}>
            <Toggle
              checked={settings.launchOnStartup}
              ariaLabel={t('settings.launchOnStartup')}
              onChange={(value) => void apply({ launchOnStartup: value })}
            />
          </SettingRow>
        </Card>
      </div>
    </div>
  )
}
