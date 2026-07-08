import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface CacheImageProps extends React.ImgHTMLAttributes<HTMLImageElement> {
  srcPath?: string;
  fallback?: React.ReactNode;
}

export const CacheImage: React.FC<CacheImageProps> = ({ srcPath, fallback, onError, ...props }) => {
  const [hasError, setHasError] = useState<boolean>(false);
  const [resolvedSrc, setResolvedSrc] = useState<string>('');

  useEffect(() => {
    let objectUrl: string | null = null;
    let cancelled = false;

    setHasError(false);
    setResolvedSrc('');

    if (!srcPath) {
      return () => {};
    }

    if (srcPath.startsWith('http://') || srcPath.startsWith('https://')) {
      const cacheRemoteImage = async () => {
        try {
          const cachedPath = await invoke<string>('cache_remote_image', { url: srcPath });
          const bytes = await invoke<number[]>('get_cache_image', { imagePath: cachedPath });
          if (!cancelled) {
            const blob = new Blob([new Uint8Array(bytes)], { type: 'image/jpeg' });
            objectUrl = URL.createObjectURL(blob);
            setResolvedSrc(objectUrl);
          }
        } catch (err) {
          console.warn('Failed to cache remote image, using original URL:', err);
          if (!cancelled) {
            setResolvedSrc(srcPath);
          }
        }
      };

      cacheRemoteImage();
      return () => {
        cancelled = true;
      };
    }

    if (!srcPath.startsWith('/cache/')) {
      setResolvedSrc(srcPath);
      return () => {};
    }

    const loadCacheImage = async () => {
      try {
        const bytes = await invoke<number[]>('get_cache_image', { imagePath: srcPath });
        if (cancelled) return;

        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/jpeg' });
        objectUrl = URL.createObjectURL(blob);
        setResolvedSrc(objectUrl);
      } catch (err) {
        console.error('Failed to load cached image:', err);
        if (!cancelled) {
          setHasError(true);
        }
      }
    };

    loadCacheImage();

    return () => {
      cancelled = true;
      if (objectUrl) {
        URL.revokeObjectURL(objectUrl);
      }
    };
  }, [srcPath]);

  const handleError = (e: React.SyntheticEvent<HTMLImageElement, Event>) => {
    setHasError(true);
    if (onError) onError(e);
  };

  if (hasError || !srcPath || !resolvedSrc) {
    if (fallback !== undefined) {
      return <>{fallback}</>;
    }
    return (
      <div
        className={props.className}
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          backgroundColor: 'var(--md-sys-color-surface-container-high)',
          color: 'var(--md-sys-color-outline)',
          ...props.style,
        }}
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          style={{ opacity: 0.6 }}
        >
          <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
          <circle cx="9" cy="9" r="2" />
          <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
        </svg>
      </div>
    );
  }

  return <img src={resolvedSrc} onError={handleError} {...props} />;
};
