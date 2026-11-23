import type { ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X } from 'lucide-react';
import { IconButton } from './actions';

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
