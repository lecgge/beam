import { useEffect, useRef } from 'react'

export interface PinInputProps {
  /** 格数,§3.3 `pin_len=4`。 */
  length?: number
  /** 当前 PIN(只含数字)。 */
  value: string
  /** 输入变化回调。 */
  onChange: (value: string) => void
  /** 禁用(例如已锁定)。 */
  disabled?: boolean
  /** 错误态(描红)。 */
  invalid?: boolean
  /** 无障碍标签。 */
  ariaLabel?: string
}

/**
 * PIN 输入格(§16 组件清单里的「PIN 输入格」)。
 *
 * 4 个格子,数字用 §16 的 PIN 字号 26 / 字重 700。
 * 交互:键盘直接输入数字、退格删一位、粘贴整串数字、当前格高亮;
 * 真实输入放在一个视觉隐藏的 input 里,这样复制粘贴与移动端键盘都能用。
 */
export function PinInput({
  length = 4,
  value,
  onChange,
  disabled = false,
  invalid = false,
  ariaLabel,
}: PinInputProps) {
  const inputRef = useRef<HTMLInputElement | null>(null)

  useEffect(() => {
    if (!disabled) inputRef.current?.focus()
  }, [disabled])

  const apply = (next: string) => {
    onChange(next.replace(/\D/g, '').slice(0, length))
  }

  return (
    <>
      <input
        ref={inputRef}
        className="pin-input-hidden"
        type="text"
        inputMode="numeric"
        autoComplete="one-time-code"
        maxLength={length}
        value={value}
        disabled={disabled}
        aria-label={ariaLabel}
        // 模态弹窗打开时优先聚焦这里(PIN 是这里唯一要操作的东西)。
        data-autofocus
        onChange={(event) => apply(event.target.value)}
        onPaste={(event) => {
          event.preventDefault()
          apply(event.clipboardData.getData('text'))
        }}
        onKeyDown={(event) => {
          // 光标已经在最前面时,退格要能删掉最后一位(受控 input 不会自动触发 change)。
          if (event.key === 'Backspace' && inputRef.current?.selectionStart === 0 && value.length > 0) {
            event.preventDefault()
            apply(value.slice(0, -1))
          }
        }}
      />
      <div className="pin-input" role="group" aria-label={ariaLabel} aria-hidden="true">
        {Array.from({ length }, (_, index) => {
          const digit = value[index]
          const active = index === value.length && !disabled
          const classes = [
            'pin-cell',
            digit ? 'filled' : '',
            active ? 'active' : '',
            invalid ? 'error' : '',
          ]
            .filter(Boolean)
            .join(' ')
          return (
            <span key={index} className={classes}>
              {digit ?? ''}
            </span>
          )
        })}
      </div>
    </>
  )
}
