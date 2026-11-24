import type { InputHTMLAttributes, KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react';
import { useId, useRef } from 'react';
import { Check } from 'lucide-react';

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

  const handleKeyDown = (e: ReactKeyboardEvent, idx: number) => {
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
