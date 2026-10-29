/** 分段选择器:设置页的分辨率 / 帧率 / 编码格式等。 */
export interface SegmentedOption<T extends string | number> {
  value: T
  label: string
}

export function SegmentedControl<T extends string | number>({
  options,
  value,
  onChange,
  disabled = false,
  ariaLabel,
}: {
  options: SegmentedOption<T>[]
  value: T
  onChange: (value: T) => void
  disabled?: boolean
  ariaLabel?: string
}) {
  return (
    <div className="segmented" role="group" aria-label={ariaLabel}>
      {options.map((option) => (
        <button
          key={String(option.value)}
          type="button"
          className={option.value === value ? 'on' : ''}
          disabled={disabled}
          aria-pressed={option.value === value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  )
}
