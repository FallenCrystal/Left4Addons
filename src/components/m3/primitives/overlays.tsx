import type { ReactNode } from 'react';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X } from 'lucide-react';
import { IconButton } from './actions';
import { useModalBehavior } from './modalBehavior';

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
