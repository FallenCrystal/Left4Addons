import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react';
import { useEffect, useId, useRef, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';

export interface SelectOption<T extends string> {
  value: T;
  label: string;
  count?: number;
  icon?: ReactNode;
  disabled?: boolean;
}

export interface SelectProps<T extends string> {
  value: T;
  options: SelectOption<T>[];
  onChange: (next: T) => void;
  ariaLabel?: string;
  icon?: ReactNode;
  variant?: 'outlined' | 'filter';
  active?: boolean;
  align?: 'start' | 'end';
  disabled?: boolean;
  className?: string;
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
  icon,
  variant = 'outlined',
  active = false,
  align = 'end',
  disabled = false,
  className = '',
}: SelectProps<T>) {
  const [open, setOpen] = useState(false);
  const selectedIndex = options.findIndex((opt) => opt.value === value);
  const [activeIndex, setActiveIndex] = useState<number>(() => Math.max(0, selectedIndex));
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listboxId = useId();

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
        triggerRef.current?.focus();
      }
    };
    window.addEventListener('mousedown', onPointerDown, true);
    window.addEventListener('keydown', onKeyDown, true);
    return () => {
      window.removeEventListener('mousedown', onPointerDown, true);
      window.removeEventListener('keydown', onKeyDown, true);
    };
  }, [open]);

  const findNextEnabledIndex = (start: number, delta: 1 | -1): number => {
    if (options.length === 0) return -1;
    let idx = start;
    for (let i = 0; i < options.length; i += 1) {
      idx = (idx + delta + options.length) % options.length;
      if (!options[idx]?.disabled) return idx;
    }
    return start;
  };

  const handleTriggerKeyDown = (e: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (disabled || options.length === 0) return;

    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        setActiveIndex(selectedIndex >= 0 ? selectedIndex : 0);
        setOpen(true);
      }
      return;
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActiveIndex((curr) => findNextEnabledIndex(curr, 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActiveIndex((curr) => findNextEnabledIndex(curr, -1));
    } else if (e.key === 'Home') {
      e.preventDefault();
      setActiveIndex(findNextEnabledIndex(options.length - 1, 1));
    } else if (e.key === 'End') {
      e.preventDefault();
      setActiveIndex(findNextEnabledIndex(0, -1));
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      const target = options[activeIndex];
      if (target && !target.disabled) {
        onChange(target.value);
        setOpen(false);
      }
    } else if (e.key === 'Tab') {
      setOpen(false);
    }
  };

  const selectedOption = options[selectedIndex] ?? options[0];
  const activeOption = options[activeIndex];

  useEffect(() => {
    if (!open || activeIndex < 0) return;
    const el = document.getElementById(`${listboxId}-opt-${activeIndex}`);
    el?.scrollIntoView?.({ block: 'nearest' });
  }, [open, activeIndex, listboxId]);

  return (
    <div
      ref={rootRef}
      className={`md-select ${className}`.trim()}
      data-open={open ? 'true' : 'false'}
      data-variant={variant}
      data-active={active ? 'true' : 'false'}
      data-align={align}
    >
      <button
        ref={triggerRef}
        type="button"
        role="combobox"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listboxId}
        aria-activedescendant={
          open && activeOption ? `${listboxId}-opt-${activeIndex}` : undefined
        }
        disabled={disabled}
        className="md-select-trigger"
        onClick={() => {
          if (disabled) return;
          if (!open) {
            setActiveIndex(selectedIndex >= 0 ? selectedIndex : 0);
          }
          setOpen((prev) => !prev);
        }}
        onKeyDown={handleTriggerKeyDown}
      >
        {variant === 'filter' && active ? (
          <Check size={14} strokeWidth={2.4} aria-hidden="true" />
        ) : (
          icon
        )}
        <span className="md-select-value">{selectedOption?.label ?? value}</span>
        {variant === 'filter' && active && selectedOption?.count !== undefined && (
          <span className="md-segmented-count">{selectedOption.count}</span>
        )}
        <ChevronDown size={15} className="md-select-caret" aria-hidden="true" />
      </button>

      {open && (
        <div id={listboxId} role="listbox" aria-label={ariaLabel} className="md-select-menu">
          {options.map((opt, idx) => {
            const isSelected = opt.value === value;
            return (
              <button
                key={opt.value}
                id={`${listboxId}-opt-${idx}`}
                type="button"
                role="option"
                tabIndex={-1}
                aria-selected={isSelected}
                disabled={opt.disabled}
                className="md-select-option"
                data-selected={isSelected ? 'true' : 'false'}
                data-highlighted={idx === activeIndex ? 'true' : 'false'}
                onMouseEnter={() => {
                  if (!opt.disabled) setActiveIndex(idx);
                }}
                onClick={() => {
                  if (opt.disabled) return;
                  onChange(opt.value);
                  setOpen(false);
                  triggerRef.current?.focus();
                }}
              >
                <span className="md-select-option-check" aria-hidden="true">
                  {isSelected ? <Check size={15} strokeWidth={2.5} /> : opt.icon}
                </span>
                <span className="md-select-option-label">{opt.label}</span>
                {opt.count !== undefined && (
                  <span className="md-select-option-count">{opt.count}</span>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
