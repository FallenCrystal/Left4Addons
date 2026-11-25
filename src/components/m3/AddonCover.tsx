import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Box,
  Crosshair,
  Eye,
  EyeOff,
  FileCode2,
  Image as ImageIcon,
  LayoutTemplate,
  Map as MapIcon,
  Music,
  Palette,
  Skull,
  User,
} from 'lucide-react';
import { isTauriRuntime, readCachedImage, workshopCoverUrl } from '../../api/ipc';
import type { CoverFilterMode } from '../../utils/contentFilter';

export type { CoverFilterMode };

const cacheUrlMap = new Map<string, string>();
const inFlightCacheLoads = new Map<string, Promise<string | null>>();

function resolveDirectUrl(src?: string): string | undefined {
  if (!src) return;
  if (/^(?:https?:)?\/\//i.test(src)) {
    const remoteUrl = src.startsWith('//') ? `https:${src}` : src;
    return isTauriRuntime() ? workshopCoverUrl(remoteUrl) : remoteUrl;
  }
  if (
    src.startsWith('data:') ||
    src.startsWith('blob:') ||
    (src.startsWith('/') && !src.includes('\\') && !/\p{Cc}/u.test(src))
  ) {
    return src;
  }
}

function isLocalCacheKey(src?: string): boolean {
  return Boolean(src && /^[a-zA-Z0-9._-]+\.(?:jpe?g|png|webp)$/i.test(src) && !src.includes('..'));
}

function mimeForCacheKey(key: string): string {
  const lower = key.toLowerCase();
  if (lower.endsWith('.png')) return 'image/png';
  if (lower.endsWith('.webp')) return 'image/webp';
  return 'image/jpeg';
}

function loadCachedImageUrl(cacheKey: string): Promise<string | null> {
  const cached = cacheUrlMap.get(cacheKey);
  if (cached) return Promise.resolve(cached);

  const existing = inFlightCacheLoads.get(cacheKey);
  if (existing) return existing;

  const promise = readCachedImage(cacheKey)
    .then((bytes) => {
      const blob = new Blob([new Uint8Array(bytes)], { type: mimeForCacheKey(cacheKey) });
      const url = URL.createObjectURL(blob);
      cacheUrlMap.set(cacheKey, url);
      return url;
    })
    .catch(() => null)
    .finally(() => {
      inFlightCacheLoads.delete(cacheKey);
    });

  inFlightCacheLoads.set(cacheKey, promise);
  return promise;
}

function hashSeed(input: string): number {
  let hash = 0;
  for (let i = 0; i < input.length; i += 1) {
    hash = (hash * 31 + input.charCodeAt(i)) >>> 0;
  }
  return hash;
}

function CategoryGlyph({ category, size }: { category?: string; size: number }) {
  switch (category) {
    case 'Campaign':
    case 'Campaigns':
    case 'Map':
    case 'Maps':
      return <MapIcon size={size} strokeWidth={1.6} />;
    case 'Survivor':
    case 'Survivors':
      return <User size={size} strokeWidth={1.6} />;
    case 'Weapon':
    case 'Weapons':
      return <Crosshair size={size} strokeWidth={1.6} />;
    case 'Audio':
    case 'Sounds':
      return <Music size={size} strokeWidth={1.6} />;
    case 'Script':
    case 'Scripts':
      return <FileCode2 size={size} strokeWidth={1.6} />;
    case 'Infected':
      return <Skull size={size} strokeWidth={1.6} />;
    case 'UI':
      return <LayoutTemplate size={size} strokeWidth={1.6} />;
    case 'Skin':
      return <Palette size={size} strokeWidth={1.6} />;
    default:
      return <Box size={size} strokeWidth={1.6} />;
  }
}

const TONAL_PAIR_VARS = [
  ['var(--md-sys-color-primary-container)', 'var(--md-sys-color-on-primary-container)'],
  ['var(--md-sys-color-secondary-container)', 'var(--md-sys-color-on-secondary-container)'],
  ['var(--md-sys-color-tertiary-container)', 'var(--md-sys-color-on-tertiary-container)'],
] as const;

export interface AddonCoverProps {
  src?: string;
  fallbackSrc?: string;
  title: string;
  category?: string;
  compact?: boolean;
  filterMode?: CoverFilterMode;
  filterLabel?: string;
  allowPeek?: boolean;
}

export function AddonCover({
  src,
  fallbackSrc,
  title,
  category,
  compact = false,
  filterMode = 'none',
  filterLabel,
  allowPeek = false,
}: AddonCoverProps) {
  const { t } = useTranslation();
  const [failedCandidates, setFailedCandidates] = useState<string[]>([]);
  const [resolvedCacheUrls, setResolvedCacheUrls] = useState<Record<string, string>>({});
  const [lazyLoadedKey, setLazyLoadedKey] = useState<string | null>(null);
  const [peekedKey, setPeekedKey] = useState<string | null>(null);

  const instanceKey = `${src ?? ''}|${fallbackSrc ?? ''}|${title}`;
  const isLazyDeferred = filterMode === 'lazy' && lazyLoadedKey !== instanceKey;
  const skipImageLoad = filterMode === 'hide' || isLazyDeferred;
  const isPeeked = allowPeek && peekedKey === instanceKey;

  const markFailed = (candidate: string) => {
    setFailedCandidates((prev) => (prev.includes(candidate) ? prev : [...prev, candidate]));
  };

  const candidates = skipImageLoad
    ? []
    : [src, fallbackSrc].filter(
        (v, idx, arr): v is string =>
          Boolean(v) &&
          arr.indexOf(v) === idx &&
          Boolean(resolveDirectUrl(v) || (isTauriRuntime() && isLocalCacheKey(v))),
      );
  const activeSrc = candidates.find((candidate) => !failedCandidates.includes(candidate));

  const directUrl = resolveDirectUrl(activeSrc);
  const shouldLoadCache = Boolean(
    !skipImageLoad && activeSrc && !directUrl && isTauriRuntime() && isLocalCacheKey(activeSrc),
  );
  const cachedImmediate = shouldLoadCache && activeSrc ? cacheUrlMap.get(activeSrc) : undefined;
  const cachedStateUrl = shouldLoadCache && activeSrc ? resolvedCacheUrls[activeSrc] : undefined;

  useEffect(() => {
    if (!shouldLoadCache || !activeSrc || cachedImmediate || cachedStateUrl) return;
    let active = true;
    void loadCachedImageUrl(activeSrc).then((url) => {
      if (!active) return;
      if (!url) {
        markFailed(activeSrc);
      } else {
        setResolvedCacheUrls((prev) =>
          prev[activeSrc] === url ? prev : { ...prev, [activeSrc]: url },
        );
      }
    });
    return () => {
      active = false;
    };
  }, [shouldLoadCache, activeSrc, cachedImmediate, cachedStateUrl]);

  if (filterMode === 'hide') {
    return (
      <div
        className="l4a-cover-filtered"
        data-filter-mode="hide"
        data-compact={compact ? 'true' : 'false'}
      >
        <EyeOff size={compact ? 15 : 22} strokeWidth={1.8} />
        {!compact && (
          <span className="l4a-cover-filter-label">{filterLabel ?? t('filter.imageHidden')}</span>
        )}
      </div>
    );
  }

  if (isLazyDeferred) {
    const lazyText = filterLabel ?? t('filter.lazyLoadPreview');
    return (
      <div
        className="l4a-cover-filtered"
        data-filter-mode="lazy"
        data-compact={compact ? 'true' : 'false'}
      >
        <button
          type="button"
          className="l4a-cover-lazy-btn"
          data-compact={compact ? 'true' : 'false'}
          aria-label={lazyText}
          onClick={(e) => {
            e.stopPropagation();
            setLazyLoadedKey(instanceKey);
          }}
        >
          <ImageIcon size={compact ? 14 : 16} />
          {!compact && <span>{lazyText}</span>}
        </button>
      </div>
    );
  }

  const resolvedUrl = directUrl ?? cachedImmediate ?? cachedStateUrl;
  const pair = TONAL_PAIR_VARS[hashSeed(title) % TONAL_PAIR_VARS.length];

  const visualContent =
    activeSrc && resolvedUrl ? (
      <img
        src={resolvedUrl}
        alt={title}
        className="l4a-cover-img"
        loading="lazy"
        draggable={false}
        onError={() => markFailed(activeSrc)}
      />
    ) : (
      <div
        className="l4a-cover-fallback"
        style={{
          backgroundColor: pair[0],
          color: pair[1],
        }}
      >
        <div
          style={{
            position: 'absolute',
            width: compact ? 42 : 120,
            height: compact ? 42 : 120,
            borderRadius: '32%',
            backgroundColor: 'currentColor',
            opacity: 0.08,
            transform: 'rotate(24deg) translate(28%, 18%)',
          }}
        />
        <div style={{ opacity: 0.85, display: 'grid', placeItems: 'center' }}>
          <CategoryGlyph category={category} size={compact ? 18 : 30} />
        </div>
      </div>
    );

  if (filterMode === 'blur') {
    const blurred = !isPeeked;
    return (
      <div
        className="l4a-cover-blur-wrap"
        data-filter-mode="blur"
        data-blurred={blurred ? 'true' : 'false'}
      >
        <div className="l4a-cover-blur-stage" data-blurred={blurred ? 'true' : 'false'}>
          {visualContent}
        </div>
        <div className="l4a-cover-blur-overlay" data-compact={compact ? 'true' : 'false'}>
          {allowPeek && !compact ? (
            <button
              type="button"
              className="l4a-cover-peek-btn"
              onClick={(e) => {
                e.stopPropagation();
                setPeekedKey(isPeeked ? null : instanceKey);
              }}
            >
              {isPeeked ? <EyeOff size={13} /> : <Eye size={13} />}
              <span>{isPeeked ? t('filter.peekHide') : t('filter.peekShow')}</span>
            </button>
          ) : blurred ? (
            <span className="md-pill" data-tone="surface">
              <EyeOff size={12} />
              {!compact && (filterLabel ?? t('filter.blurredImage'))}
            </span>
          ) : null}
        </div>
      </div>
    );
  }

  return visualContent;
}
