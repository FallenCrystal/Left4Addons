import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, X } from 'lucide-react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'filled' | 'tonal' | 'outlined' | 'text' | 'elevated';
  tone?: 'default' | 'error';
  size?: 'md' | 'sm';
  icon?: ReactNode;
}

export function Button({
  variant = 'filled',
  tone = 'default',
  size = 'md',
  icon,
  children,
  className = '',
  type = 'button',
  title,
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={`md-btn ${className}`.trim()}
      data-variant={variant}
      data-tone={tone}
      data-size={size}
      data-tooltip={title}
      {...rest}
    >
      {icon}
      {children}
    </button>
  );
}

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'standard' | 'tonal' | 'filled' | 'outlined';
  size?: 'md' | 'sm';
  active?: boolean;
  label: string;
}

export function IconButton({
  variant = 'standard',
  size = 'md',
  active = false,
  label,
  title,
  className = '',
  type = 'button',
  children,
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type}
      aria-label={label}
      data-tooltip={title ?? label}
      className={`md-icon-btn ${className}`.trim()}
      data-variant={variant}
      data-size={size}
      data-active={active ? 'true' : 'false'}
      {...rest}
    >
      {children}
    </button>
  );
}

export interface FilterChipProps {
  label: string;
  selected?: boolean;
  count?: number;
  icon?: ReactNode;
  tone?: 'default' | 'tertiary' | 'error' | 'surface';
  onClick?: () => void;
  onRemove?: () => void;
  removeLabel?: string;
}

export function FilterChip({
  label,
  selected = false,
  count,
  icon,
  tone = 'default',
  onClick,
  onRemove,
  removeLabel,
}: FilterChipProps) {
  const { t } = useTranslation();

  if (onRemove) {
    return (
      <span
        className="md-chip md-chip-removable"
        data-selected={selected ? 'true' : 'false'}
        data-tone={tone}
      >
        <button
          type="button"
          className="md-chip-main"
          onClick={onClick}
        >
          {selected ? <Check size={14} strokeWidth={2.4} /> : icon}
          <span>{label}</span>
          {count !== undefined && <span className="md-segmented-count">{count}</span>}
        </button>
        <button
          type="button"
          className="md-chip-remove"
          aria-label={removeLabel ?? `${t('common.remove')} ${label}`}
          onClick={onRemove}
        >
          <X size={13} />
        </button>
      </span>
    );
  }

  return (
    <button
      type="button"
      className="md-chip"
      aria-pressed={selected}
      data-selected={selected ? 'true' : 'false'}
      data-tone={tone}
      onClick={onClick}
    >
      {selected ? <Check size={14} strokeWidth={2.4} /> : icon}
      <span>{label}</span>
      {count !== undefined && <span className="md-segmented-count">{count}</span>}
    </button>
  );
}

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  count?: number;
  icon?: ReactNode;
  disabled?: boolean;
}

export interface SegmentedButtonProps<T extends string> {
  value: T;
  options: SegmentedOption<T>[];
  onChange: (next: T) => void;
  ariaLabel?: string;
}

export function SegmentedButton<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
}: SegmentedButtonProps<T>) {
  return (
    <div className="md-segmented" role="group" aria-label={ariaLabel}>
      {options.map((opt) => {
        const selected = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            className="md-segmented-item"
            data-selected={selected ? 'true' : 'false'}
            aria-pressed={selected}
            disabled={opt.disabled}
            onClick={() => onChange(opt.value)}
          >
            {selected ? <Check size={14} strokeWidth={2.4} /> : opt.icon}
            <span>{opt.label}</span>
            {opt.count !== undefined && (
              <span className="md-segmented-count">{opt.count}</span>
            )}
          </button>
        );
      })}
    </div>
  );
}
