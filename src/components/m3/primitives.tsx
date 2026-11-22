import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, RefObject } from 'react';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, ChevronDown, X } from 'lucide-react';

const FOCUSABLE_SELECTOR =
  'button:not([disabled]):not([tabindex="-1"]), [href]:not([tabindex="-1"]), input:not([disabled]):not([tabindex="-1"]), select:not([disabled]):not([tabindex="-1"]), textarea:not([disabled]):not([tabindex="-1"]), [tabindex]:not([tabindex="-1"])';

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

export interface SelectOption<T extends string> {
  value: T;
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
}

export interface SelectProps<T extends string> {
  value: T;
  options: SelectOption<T>[];
  onChange: (next: T) => void;
  ariaLabel?: string;
  icon?: ReactNode;
  disabled?: boolean;
  className?: string;
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
  icon,
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

  const handleTriggerKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
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

  return (
    <div
      ref={rootRef}
      className={`md-select ${className}`.trim()}
      data-open={open ? 'true' : 'false'}
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
          open && activeOption ? `${listboxId}-opt-${activeOption.value}` : undefined
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
        {icon}
        <span className="md-select-value">{selectedOption?.label ?? value}</span>
        <ChevronDown size={15} className="md-select-caret" aria-hidden="true" />
      </button>

      {open && (
        <div id={listboxId} role="listbox" aria-label={ariaLabel} className="md-select-menu">
          {options.map((opt, idx) => {
            const isSelected = opt.value === value;
            return (
              <button
                key={opt.value}
                id={`${listboxId}-opt-${opt.value}`}
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
              </button>
            );
          })}
        </div>
      )}
    </div>
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

const M3E_SAMPLES = 72;
const M3E_CENTER = 24;
const M3E_STEP_MS = 650;

function roundedNgonRadius(
  theta: number,
  sides: number,
  vertexRadius: number,
  cornerRadius: number,
  rotation = -Math.PI / 2,
): number {
  const step = (2 * Math.PI) / sides;
  const half = Math.PI / sides;
  let rel = (theta - rotation) % step;
  if (rel < 0) rel += step;
  const delta = Math.abs(rel - half);
  const phi = half - delta;
  const faceDist = vertexRadius * Math.cos(half) + cornerRadius;
  const maxTan = (vertexRadius * Math.sin(half)) / faceDist;
  if (delta < half - 1e-6 && Math.tan(delta) <= maxTan) {
    return faceDist / Math.cos(delta);
  }
  const perp = vertexRadius * Math.sin(phi);
  return (
    vertexRadius * Math.cos(phi) +
    Math.sqrt(Math.max(0, cornerRadius * cornerRadius - perp * perp))
  );
}

const M3E_RADIUS_FNS: ReadonlyArray<(theta: number) => number> = [
  // 0: SoftBurst (10-pointed soft starburst)
  (t) => 16.6 + 2.5 * (Math.cos(10 * t) + 0.16 * Math.cos(20 * t)),
  // 1: Cookie9Sided (9-lobed scalloped cookie)
  (t) => 17.2 + 2.1 * (Math.cos(9 * t) - 0.22 * Math.cos(18 * t)),
  // 2: Pentagon (rounded regular pentagon)
  (t) => roundedNgonRadius(t, 5, 11.4, 7.6, -Math.PI / 2),
  // 3: Pill (rounded stadium capsule)
  (t) => roundedNgonRadius(t, 2, 6.2, 13.0, -Math.PI / 4),
  // 4: Sunny (8-pointed soft sun)
  (t) => 16.8 + 2.4 * (Math.cos(8 * t) + 0.1 * Math.cos(16 * t)),
  // 5: Cookie4Sided (4-lobed clover cookie)
  (t) =>
    16.2 +
    3.1 *
      (Math.cos(4 * (t - Math.PI / 4)) - 0.2 * Math.cos(8 * (t - Math.PI / 4))),
  // 6: Oval (expressive ellipse)
  (t) => {
    const a = 19.2;
    const b = 14.4;
    const c = Math.cos(t - Math.PI / 4);
    const s = Math.sin(t - Math.PI / 4);
    return (a * b) / Math.hypot(b * c, a * s);
  },
];

// Precompute cubic Bezier control points (startX, startY, followed by N * 6 floats: c1x, c1y, c2x, c2y, ex, ey)
const M3E_BEZIER_SHAPES: ReadonlyArray<Float32Array> = M3E_RADIUS_FNS.map((fn) => {
  const ptsX = new Float32Array(M3E_SAMPLES);
  const ptsY = new Float32Array(M3E_SAMPLES);
  for (let i = 0; i < M3E_SAMPLES; i += 1) {
    const theta = (2 * Math.PI * i) / M3E_SAMPLES;
    const r = fn(theta);
    ptsX[i] = M3E_CENTER + r * Math.cos(theta);
    ptsY[i] = M3E_CENTER + r * Math.sin(theta);
  }
  const out = new Float32Array(2 + M3E_SAMPLES * 6);
  out[0] = ptsX[0];
  out[1] = ptsY[0];
  for (let i = 0; i < M3E_SAMPLES; i += 1) {
    const i0 = (i - 1 + M3E_SAMPLES) % M3E_SAMPLES;
    const i1 = i;
    const i2 = (i + 1) % M3E_SAMPLES;
    const i3 = (i + 2) % M3E_SAMPLES;
    const base = 2 + i * 6;
    out[base] = ptsX[i1] + (ptsX[i2] - ptsX[i0]) / 6;
    out[base + 1] = ptsY[i1] + (ptsY[i2] - ptsY[i0]) / 6;
    out[base + 2] = ptsX[i2] - (ptsX[i3] - ptsX[i1]) / 6;
    out[base + 3] = ptsY[i2] - (ptsY[i3] - ptsY[i1]) / 6;
    out[base + 4] = ptsX[i2];
    out[base + 5] = ptsY[i2];
  }
  return out;
});

function formatMorphPath(shapeA: Float32Array, shapeB: Float32Array, u: number): string {
  const v = 1 - u;
  const sx = (shapeA[0] * v + shapeB[0] * u).toFixed(2);
  const sy = (shapeA[1] * v + shapeB[1] * u).toFixed(2);
  let d = `M${sx} ${sy}`;
  for (let i = 0; i < M3E_SAMPLES; i += 1) {
    const base = 2 + i * 6;
    const c1x = (shapeA[base] * v + shapeB[base] * u).toFixed(2);
    const c1y = (shapeA[base + 1] * v + shapeB[base + 1] * u).toFixed(2);
    const c2x = (shapeA[base + 2] * v + shapeB[base + 2] * u).toFixed(2);
    const c2y = (shapeA[base + 3] * v + shapeB[base + 3] * u).toFixed(2);
    const ex = (shapeA[base + 4] * v + shapeB[base + 4] * u).toFixed(2);
    const ey = (shapeA[base + 5] * v + shapeB[base + 5] * u).toFixed(2);
    d += `C${c1x} ${c1y} ${c2x} ${c2y} ${ex} ${ey}`;
  }
  return `${d}Z`;
}

const M3E_INITIAL_PATH = formatMorphPath(M3E_BEZIER_SHAPES[0], M3E_BEZIER_SHAPES[0], 0);

function m3eSpringEase(p: number): number {
  if (p <= 0) return 0;
  if (p >= 0.88) return 1;
  const t = p / 0.88;
  const raw = (x: number) =>
    1 - Math.exp(-5.6 * x) * (Math.cos(4.4 * x) + 0.55 * Math.sin(4.4 * x));
  const end = raw(1);
  return raw(t) + (1 - end) * (t * t * (3 - 2 * t));
}

export function LoadingIndicator({
  size = 'md',
  contained = false,
  label,
}: {
  size?: 'sm' | 'md' | 'lg';
  contained?: boolean;
  label?: string;
}) {
  const svgRef = useRef<SVGSVGElement>(null);
  const pathRef = useRef<SVGPathElement>(null);

  useEffect(() => {
    if (
      typeof window === 'undefined' ||
      window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
    ) {
      return;
    }

    const shapeCount = M3E_BEZIER_SHAPES.length;
    const stepAngle = 360 / shapeCount;
    let frameId = 0;
    const startTime = performance.now();

    const tick = (now: number) => {
      const elapsed = Math.max(0, now - startTime);
      const totalSteps = elapsed / M3E_STEP_MS;
      const stepIndex = Math.floor(totalSteps);
      const stepProgress = totalSteps - stepIndex;
      const eased = m3eSpringEase(stepProgress);

      const fromShape = M3E_BEZIER_SHAPES[stepIndex % shapeCount];
      const toShape = M3E_BEZIER_SHAPES[(stepIndex + 1) % shapeCount];

      if (pathRef.current) {
        pathRef.current.setAttribute('d', formatMorphPath(fromShape, toShape, eased));
      }
      if (svgRef.current) {
        const angle = ((stepIndex + eased) * stepAngle) % 360;
        const pulse =
          stepProgress < 0.72
            ? 1 - 0.05 * Math.sin((Math.PI * stepProgress) / 0.72)
            : 1;
        svgRef.current.style.transform = `rotate(${angle.toFixed(2)}deg) scale(${pulse.toFixed(4)})`;
      }

      frameId = window.requestAnimationFrame(tick);
    };

    frameId = window.requestAnimationFrame(tick);
    return () => {
      window.cancelAnimationFrame(frameId);
    };
  }, []);

  return (
    <span
      className="md-loading-indicator"
      role="progressbar"
      aria-label={label}
      data-size={size}
      data-contained={contained ? 'true' : 'false'}
    >
      <span className="md-loading-rotor" aria-hidden="true">
        <svg
          ref={svgRef}
          className="md-loading-shape"
          viewBox="0 0 48 48"
          fill="currentColor"
          shapeRendering="geometricPrecision"
        >
          <path ref={pathRef} d={M3E_INITIAL_PATH} />
        </svg>
      </span>
    </span>
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

export interface SnackbarData {
  id: number;
  message: string;
  tone?: 'default' | 'error';
  actionLabel?: string;
  onAction?: () => void;
}

export function Snackbar({
  item,
  onClose,
}: {
  item: SnackbarData | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [displayed, setDisplayed] = useState<SnackbarData | null>(item);

  if (item && item.id !== displayed?.id) {
    setDisplayed(item);
  }

  useEffect(() => {
    if (item || !displayed) return;
    const timer = window.setTimeout(() => {
      setDisplayed(null);
    }, 200);
    return () => window.clearTimeout(timer);
  }, [item, displayed]);

  const active = item ?? displayed;
  if (!active) return null;

  const tone = active.tone ?? 'default';
  const state = item ? 'open' : 'closing';

  return (
    <div
      key={active.id}
      className="md-snackbar"
      role="status"
      aria-live={tone === 'error' ? 'assertive' : 'polite'}
      data-tone={tone}
      data-state={state}
    >
      <span className="md-snackbar-text">{active.message}</span>
      {active.actionLabel && active.onAction && (
        <button
          type="button"
          className="md-snackbar-action"
          onClick={() => {
            active.onAction?.();
            onClose();
          }}
        >
          {active.actionLabel}
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

export function TooltipHost() {
  const [active, setActive] = useState<{
    text: string;
    rect: DOMRect;
  } | null>(null);
  const [coords, setCoords] = useState<{ top: number; left: number; placement: 'top' | 'bottom' }>({
    top: -9999,
    left: -9999,
    placement: 'bottom',
  });
  const tooltipRef = useRef<HTMLDivElement>(null);
  const targetRef = useRef<HTMLElement | null>(null);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    const clearTimer = () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };

    const hide = () => {
      clearTimer();
      if (targetRef.current !== null) {
        targetRef.current = null;
        setActive((prev) => (prev ? null : prev));
      } else {
        setActive((prev) => (prev ? null : prev));
      }
    };

    const showForTarget = (el: HTMLElement, delayMs: number) => {
      const text = el.getAttribute('data-tooltip')?.trim();
      if (!text || el.hasAttribute('disabled') || el.getAttribute('aria-disabled') === 'true') {
        hide();
        return;
      }
      clearTimer();
      targetRef.current = el;
      timerRef.current = window.setTimeout(() => {
        if (!el.isConnected) {
          hide();
          return;
        }
        const latestText = el.getAttribute('data-tooltip')?.trim();
        if (!latestText) {
          hide();
          return;
        }
        setActive({ text: latestText, rect: el.getBoundingClientRect() });
      }, delayMs);
    };

    const onPointerOver = (e: PointerEvent) => {
      if (e.pointerType === 'touch') return;
      const el = (e.target as Element | null)?.closest?.('[data-tooltip]') as HTMLElement | null;
      if (!el) {
        if (targetRef.current) hide();
        return;
      }
      if (el === targetRef.current) return;
      showForTarget(el, targetRef.current ? 120 : 360);
    };

    const onPointerOut = (e: PointerEvent) => {
      if (!targetRef.current) return;
      const related = e.relatedTarget as Node | null;
      if (related && targetRef.current.contains(related)) return;
      hide();
    };

    const onFocusIn = (e: FocusEvent) => {
      const el = (e.target as Element | null)?.closest?.('[data-tooltip]') as HTMLElement | null;
      if (!el || !el.matches(':focus-visible')) return;
      showForTarget(el, 150);
    };

    window.addEventListener('pointerover', onPointerOver, true);
    window.addEventListener('pointerout', onPointerOut, true);
    window.addEventListener('focusin', onFocusIn, true);
    window.addEventListener('focusout', hide, true);
    window.addEventListener('pointerdown', hide, true);
    window.addEventListener('keydown', hide, true);
    window.addEventListener('wheel', hide, { passive: true, capture: true });
    window.addEventListener('scroll', hide, true);
    window.addEventListener('blur', hide);

    return () => {
      clearTimer();
      window.removeEventListener('pointerover', onPointerOver, true);
      window.removeEventListener('pointerout', onPointerOut, true);
      window.removeEventListener('focusin', onFocusIn, true);
      window.removeEventListener('focusout', hide, true);
      window.removeEventListener('pointerdown', hide, true);
      window.removeEventListener('keydown', hide, true);
      window.removeEventListener('wheel', hide, true);
      window.removeEventListener('scroll', hide, true);
      window.removeEventListener('blur', hide);
    };
  }, []);

  useLayoutEffect(() => {
    if (!active || !tooltipRef.current) return;
    const tipRect = tooltipRef.current.getBoundingClientRect();
    const margin = 8;
    const gap = 8;
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    let left = active.rect.left + active.rect.width / 2 - tipRect.width / 2;
    left = Math.max(margin, Math.min(vw - tipRect.width - margin, left));

    let placement: 'top' | 'bottom' = 'bottom';
    let top = active.rect.bottom + gap;
    if (top + tipRect.height + margin > vh && active.rect.top - gap - tipRect.height >= margin) {
      placement = 'top';
      top = active.rect.top - gap - tipRect.height;
    }

    setCoords({ top, left, placement });
  }, [active]);

  if (!active) return null;

  return (
    <div
      ref={tooltipRef}
      role="tooltip"
      className="md-tooltip"
      data-placement={coords.placement}
      style={{
        top: `${coords.top}px`,
        left: `${coords.left}px`,
      }}
    >
      {active.text}
    </div>
  );
}
