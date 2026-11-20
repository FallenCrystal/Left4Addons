import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, RefObject } from 'react';
import { useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, X } from 'lucide-react';

const FOCUSABLE_SELECTOR =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

const overlayStack: symbol[] = [];

function useModalBehavior(
  open: boolean,
  containerRef: RefObject<HTMLElement | null>,
  onClose: () => void,
) {
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    if (!open) return;
    const token = Symbol('overlay');
    overlayStack.push(token);

    const previouslyFocused =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const frame = window.requestAnimationFrame(() => {
      const container = containerRef.current;
      if (!container) return;
      if (!container.contains(document.activeElement)) {
        const autoFocusEl = container.querySelector<HTMLElement>('[autofocus]');
        const firstFocusable = autoFocusEl ?? container.querySelector<HTMLElement>(FOCUSABLE_SELECTOR);
        (firstFocusable ?? container).focus();
      }
    });

    const onKeyDown = (e: KeyboardEvent) => {
      if (overlayStack[overlayStack.length - 1] !== token) return;

      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onCloseRef.current();
        return;
      }

      if (e.key === 'Tab') {
        const container = containerRef.current;
        if (!container) return;
        const focusables = Array.from(
          container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
        ).filter((el) => el.offsetParent !== null);
        if (focusables.length === 0) {
          e.preventDefault();
          container.focus();
          return;
        }
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        if (e.shiftKey) {
          if (document.activeElement === first || !container.contains(document.activeElement)) {
            e.preventDefault();
            last.focus();
          }
        } else if (
          document.activeElement === last ||
          !container.contains(document.activeElement)
        ) {
          e.preventDefault();
          first.focus();
        }
      }
    };

    window.addEventListener('keydown', onKeyDown, true);
    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener('keydown', onKeyDown, true);
      const idx = overlayStack.lastIndexOf(token);
      if (idx >= 0) overlayStack.splice(idx, 1);
      if (previouslyFocused && previouslyFocused.isConnected) {
        previouslyFocused.focus();
      }
    };
  }, [open, containerRef]);
}

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
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={`md-btn ${className}`.trim()}
      data-variant={variant}
      data-tone={tone}
      data-size={size}
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
  className = '',
  type = 'button',
  children,
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type}
      aria-label={label}
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

export interface SwitchProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  label: string;
  compact?: boolean;
}

export function Switch({ checked, onChange, disabled, label, compact }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className="md-switch"
      data-checked={checked ? 'true' : 'false'}
      data-size={compact ? 'compact' : 'default'}
      onClick={(e) => {
        e.stopPropagation();
        if (!disabled) onChange(!checked);
      }}
    >
      <span className="md-switch-thumb">
        {checked && <Check size={compact ? 11 : 14} strokeWidth={2.6} />}
      </span>
    </button>
  );
}

export interface CheckboxProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
}

export function Checkbox({ checked, onChange, label }: CheckboxProps) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      aria-label={label}
      className="md-checkbox"
      data-checked={checked ? 'true' : 'false'}
      onClick={(e) => {
        e.stopPropagation();
        onChange(!checked);
      }}
    >
      {checked && <Check size={13} strokeWidth={3} />}
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

export interface TextFieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  trailing?: ReactNode;
}

export function TextField({ label, trailing, id, ...rest }: TextFieldProps) {
  const autoId = useId();
  const fieldId = id ?? autoId;
  return (
    <label className="md-field" htmlFor={fieldId}>
      <span className="md-field-label">{label}</span>
      <span className="md-field-control">
        <input id={fieldId} className="md-field-input" {...rest} />
        {trailing}
      </span>
    </label>
  );
}

export interface TabItem<T extends string> {
  value: T;
  label: string;
  count?: number;
  icon?: ReactNode;
}

export function Tabs<T extends string>({
  value,
  tabs,
  onChange,
  compact = false,
  idPrefix,
}: {
  value: T;
  tabs: TabItem<T>[];
  onChange: (next: T) => void;
  compact?: boolean;
  idPrefix?: string;
}) {
  const autoPrefix = useId();
  const prefix = idPrefix ?? autoPrefix;
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);

  const handleKeyDown = (e: React.KeyboardEvent, idx: number) => {
    let nextIdx: number | null = null;
    if (e.key === 'ArrowRight') nextIdx = (idx + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') nextIdx = (idx - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') nextIdx = 0;
    else if (e.key === 'End') nextIdx = tabs.length - 1;

    if (nextIdx !== null) {
      e.preventDefault();
      onChange(tabs[nextIdx].value);
      tabRefs.current[nextIdx]?.focus();
    }
  };

  return (
    <div className="md-tabs" role="tablist" data-compact={compact ? 'true' : 'false'}>
      {tabs.map((tab, idx) => {
        const active = tab.value === value;
        return (
          <button
            key={tab.value}
            ref={(el) => {
              tabRefs.current[idx] = el;
            }}
            id={`${prefix}-tab-${tab.value}`}
            type="button"
            role="tab"
            tabIndex={active ? 0 : -1}
            aria-selected={active}
            aria-controls={`${prefix}-panel-${tab.value}`}
            className="md-tab"
            data-active={active ? 'true' : 'false'}
            onKeyDown={(e) => handleKeyDown(e, idx)}
            onClick={() => onChange(tab.value)}
          >
            {tab.icon}
            <span>{tab.label}</span>
            {tab.count !== undefined && (
              <span className="md-pill" data-tone={active ? 'primary' : 'surface'}>
                {tab.count}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

export function LinearProgress({
  value,
  indeterminate = false,
  label,
}: {
  value?: number;
  indeterminate?: boolean;
  label?: string;
}) {
  const clamped = Math.max(0, Math.min(100, value ?? 0));
  return (
    <div
      className="md-progress"
      role="progressbar"
      aria-label={label}
      aria-valuenow={indeterminate ? undefined : clamped}
      aria-valuemin={0}
      aria-valuemax={100}
      data-indeterminate={indeterminate ? 'true' : 'false'}
    >
      <div
        className="md-progress-bar"
        style={indeterminate ? undefined : { width: `${clamped}%` }}
      />
    </div>
  );
}

export function EmptyState({
  icon,
  title,
  action,
}: {
  icon: ReactNode;
  title: string;
  action?: ReactNode;
}) {
  return (
    <div className="md-empty">
      <div className="md-empty-icon">{icon}</div>
      <div className="md-title-medium">{title}</div>
      {action}
    </div>
  );
}

export function Dialog({
  open,
  title,
  icon,
  wide = false,
  onClose,
  children,
  actions,
}: {
  open: boolean;
  title: string;
  icon?: ReactNode;
  wide?: boolean;
  onClose: () => void;
  children: ReactNode;
  actions?: ReactNode;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useModalBehavior(open, dialogRef, onClose);

  if (!open) return null;
  return (
    <div className="md-scrim" data-center="true" onClick={onClose} role="presentation">
      <div
        ref={dialogRef}
        tabIndex={-1}
        className="md-dialog"
        data-wide={wide ? 'true' : 'false'}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="md-dialog-header">
          {icon}
          <h2 id={titleId} className="md-headline-small" style={{ fontSize: 20, lineHeight: '28px' }}>
            {title}
          </h2>
        </div>
        <div className="md-dialog-body">{children}</div>
        {actions && <div className="md-dialog-actions">{actions}</div>}
      </div>
    </div>
  );
}

export function SideSheet({
  open,
  title,
  onClose,
  headerTrailing,
  children,
  footer,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  headerTrailing?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const { t } = useTranslation();
  const sheetRef = useRef<HTMLElement>(null);
  const titleId = useId();
  useModalBehavior(open, sheetRef, onClose);

  if (!open) return null;
  return (
    <div className="md-scrim" onClick={onClose} role="presentation">
      <aside
        ref={sheetRef}
        tabIndex={-1}
        className="md-side-sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="md-sheet-header">
          <h2 id={titleId} className="md-title-large l4a-truncate" style={{ flex: 1 }}>
            {title}
          </h2>
          {headerTrailing}
          <IconButton label={t('common.close')} onClick={onClose}>
            <X size={20} />
          </IconButton>
        </div>
        {children}
        {footer && <div className="md-sheet-footer">{footer}</div>}
      </aside>
    </div>
  );
}

export function Snackbar({
  message,
  tone = 'default',
  actionLabel,
  onAction,
  onClose,
}: {
  message: string;
  tone?: 'default' | 'error';
  actionLabel?: string;
  onAction?: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="md-snackbar" role="status" data-tone={tone}>
      <span style={{ flex: 1 }}>{message}</span>
      {actionLabel && onAction && (
        <button
          type="button"
          className="md-snackbar-action"
          onClick={() => {
            onAction();
            onClose();
          }}
        >
          {actionLabel}
        </button>
      )}
      <IconButton
        label={t('common.dismiss')}
        size="sm"
        onClick={onClose}
        style={{ color: 'inherit' }}
      >
        <X size={16} />
      </IconButton>
    </div>
  );
}
