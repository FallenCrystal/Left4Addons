import { useEffect, useState } from 'react';
import {
  Box,
  Crosshair,
  FileCode2,
  LayoutTemplate,
  Map as MapIcon,
  Music,
  Palette,
  Skull,
  User,
} from 'lucide-react';
import { isTauriRuntime, readCachedImage, workshopCoverUrl } from '../../api/ipc';

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

export function AddonCover({
  src,
  fallbackSrc,
  title,
  category,
  compact = false,
}: {
  src?: string;
  fallbackSrc?: string;
  title: string;
  category?: string;
  compact?: boolean;
}) {
  const [failedCandidates, setFailedCandidates] = useState<string[]>([]);
  const [resolvedCacheUrls, setResolvedCacheUrls] = useState<Record<string, string>>({});

  const markFailed = (candidate: string) => {
    setFailedCandidates((prev) => (prev.includes(candidate) ? prev : [...prev, candidate]));
  };

  const candidates = [src, fallbackSrc].filter(
    (v, idx, arr): v is string =>
      Boolean(v) &&
      arr.indexOf(v) === idx &&
      Boolean(resolveDirectUrl(v) || (isTauriRuntime() && isLocalCacheKey(v))),
  );
  const activeSrc = candidates.find((candidate) => !failedCandidates.includes(candidate));

  const directUrl = resolveDirectUrl(activeSrc);
  const shouldLoadCache = Boolean(
    activeSrc && !directUrl && isTauriRuntime() && isLocalCacheKey(activeSrc),
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

  const resolvedUrl = directUrl ?? cachedImmediate ?? cachedStateUrl;

  if (activeSrc && resolvedUrl) {
    return (
      <img
        src={resolvedUrl}
        alt={title}
        className="l4a-cover-img"
        loading="lazy"
        draggable={false}
        onError={() => markFailed(activeSrc)}
      />
    );
  }

  const pair = TONAL_PAIR_VARS[hashSeed(title) % TONAL_PAIR_VARS.length];
  return (
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
}
