import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface CacheImageProps extends React.ImgHTMLAttributes<HTMLImageElement> {
  srcPath?: string;
  fallback?: React.ReactNode;
}

export const CacheImage: React.FC<CacheImageProps> = ({ srcPath, fallback, onError, ...props }) => {
  const [dataUrl, setDataUrl] = useState<string>('');
  const [hasError, setHasError] = useState<boolean>(false);

  useEffect(() => {
    let urlToRevoke: string | null = null;
    let mounted = true;

    const loadImg = async () => {
      if (!srcPath) {
        setHasError(true);
        return;
      }
      if (srcPath.startsWith('http://') || srcPath.startsWith('https://')) {
        if (mounted) setDataUrl(srcPath);
        return;
      }
      if (srcPath.startsWith('/cache/')) {
        const filename = srcPath.slice(7);
        try {
          const bytes = await invoke<number[]>('read_cache_image', { filename });
          const blob = new Blob([new Uint8Array(bytes)], { type: 'image/jpeg' });
          const url = URL.createObjectURL(blob);
          if (mounted) {
            setDataUrl(url);
            urlToRevoke = url;
          } else {
            URL.revokeObjectURL(url);
          }
        } catch (e) {
          console.error('Failed to load cache image:', e);
          if (mounted) setHasError(true);
        }
      } else {
        if (mounted) setDataUrl(srcPath);
      }
    };

    loadImg();

    return () => {
      mounted = false;
      if (urlToRevoke) {
        URL.revokeObjectURL(urlToRevoke);
      }
    };
  }, [srcPath]);

  const handleError = (e: React.SyntheticEvent<HTMLImageElement, Event>) => {
    setHasError(true);
    if (onError) onError(e);
  };

  if (hasError || !dataUrl) {
    return <>{fallback}</>;
  }

  return <img src={dataUrl} onError={handleError} {...props} />;
};
