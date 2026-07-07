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
    return <>{fallback}</>;
  }

  return <img src={resolvedSrc} onError={handleError} {...props} />;
};
