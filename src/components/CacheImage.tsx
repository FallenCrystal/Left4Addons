import React, { useState, useEffect } from 'react';
import { getImageUrl } from '../utils/addonHelpers';

interface CacheImageProps extends React.ImgHTMLAttributes<HTMLImageElement> {
  srcPath?: string;
  fallback?: React.ReactNode;
}

export const CacheImage: React.FC<CacheImageProps> = ({ srcPath, fallback, onError, ...props }) => {
  const [hasError, setHasError] = useState<boolean>(false);

  useEffect(() => {
    setHasError(false);
  }, [srcPath]);

  const handleError = (e: React.SyntheticEvent<HTMLImageElement, Event>) => {
    setHasError(true);
    if (onError) onError(e);
  };

  if (hasError || !srcPath) {
    return <>{fallback}</>;
  }

  const url = getImageUrl(srcPath);

  return <img src={url} onError={handleError} {...props} />;
};

