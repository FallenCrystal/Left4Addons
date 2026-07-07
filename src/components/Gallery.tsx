import React, { useState, useEffect, useRef } from 'react';
import { ChevronLeft, ChevronRight, X, FileText } from 'lucide-react';
import { CacheImage } from './CacheImage';

interface GalleryProps {
  gallery: string[];
  title: string;
  fallbackImage?: string;
  fallbackIcon?: React.ReactNode;
}

export const Gallery: React.FC<GalleryProps> = ({
  gallery,
  title,
  fallbackImage,
  fallbackIcon,
}) => {
  const [galleryIndex, setGalleryIndex] = useState(0);
  const [fullscreenImageIndex, setFullscreenImageIndex] = useState<number | null>(null);
  const thumbnailContainerRef = useRef<HTMLDivElement>(null);
  const hasGallery = gallery.length > 0;

  // Scroll thumbnail into view when galleryIndex changes
  useEffect(() => {
    if (thumbnailContainerRef.current) {
      const container = thumbnailContainerRef.current;
      const activeThumb = container.children[galleryIndex] as HTMLElement;
      if (activeThumb) {
        const containerRect = container.getBoundingClientRect();
        const thumbRect = activeThumb.getBoundingClientRect();
        if (thumbRect.left < containerRect.left || thumbRect.right > containerRect.right) {
          activeThumb.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'center' });
        }
      }
    }
  }, [galleryIndex]);

  // Reset indices if gallery changes (e.g. switching items)
  useEffect(() => {
    setGalleryIndex(0);
    setFullscreenImageIndex(null);
  }, [gallery]);

  return (
    <>
      <div className="detail-image-box" style={{ position: 'relative', overflow: 'hidden' }}>
        {hasGallery ? (
          <>
            <div style={{ position: 'relative', width: '100%', height: '100%', background: '#000' }}>
              {gallery.map((url, i) => (
                <div
                  key={url + '-' + i}
                  onClick={() => setFullscreenImageIndex(i)}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    right: 0,
                    bottom: 0,
                    opacity: i === galleryIndex ? 1 : 0,
                    transition: 'opacity 0.3s ease-in-out',
                    pointerEvents: i === galleryIndex ? 'auto' : 'none',
                    zIndex: i === galleryIndex ? 1 : 0,
                    cursor: 'zoom-in',
                  }}
                >
                  <CacheImage
                    srcPath={url}
                    alt={title}
                    className="detail-image"
                    style={{ width: '100%', height: '100%', objectFit: 'contain' }}
                  />
                </div>
              ))}
            </div>
            {gallery.length > 1 && (
              <>
                <button
                  onClick={() => setGalleryIndex((i) => (i - 1 + gallery.length) % gallery.length)}
                  style={{
                    position: 'absolute',
                    left: 8,
                    top: '50%',
                    transform: 'translateY(-50%)',
                    background: 'rgba(0,0,0,0.6)',
                    border: 'none',
                    borderRadius: '50%',
                    width: 28,
                    height: 28,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    cursor: 'pointer',
                    color: '#fff',
                    zIndex: 2,
                  }}
                >
                  <ChevronLeft size={16} />
                </button>
                <button
                  onClick={() => setGalleryIndex((i) => (i + 1) % gallery.length)}
                  style={{
                    position: 'absolute',
                    right: 8,
                    top: '50%',
                    transform: 'translateY(-50%)',
                    background: 'rgba(0,0,0,0.6)',
                    border: 'none',
                    borderRadius: '50%',
                    width: 28,
                    height: 28,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    cursor: 'pointer',
                    color: '#fff',
                    zIndex: 2,
                  }}
                >
                  <ChevronRight size={16} />
                </button>
                <div
                  style={{
                    position: 'absolute',
                    bottom: 8,
                    right: 8,
                    background: 'rgba(0,0,0,0.6)',
                    borderRadius: 8,
                    padding: '2px 8px',
                    fontSize: '11px',
                    color: '#fff',
                    zIndex: 2,
                  }}
                >
                  {galleryIndex + 1} / {gallery.length}
                </div>
              </>
            )}
          </>
        ) : fallbackImage ? (
          <CacheImage
            srcPath={fallbackImage}
            alt={title}
            className="detail-image"
            fallback={fallbackIcon || <FileText size={64} className="text-secondary" />}
          />
        ) : (
          fallbackIcon || <FileText size={64} className="text-secondary" />
        )}
      </div>

      {/* Gallery thumbnails */}
      {hasGallery && gallery.length > 1 && (
        <div
          ref={thumbnailContainerRef}
          style={{
            display: 'flex',
            gap: '4px',
            overflowX: 'auto',
            marginTop: '8px',
            paddingBottom: '4px',
            scrollBehavior: 'smooth',
          }}
        >
          {gallery.map((url, i) => (
            <CacheImage
              key={i}
              srcPath={url}
              alt=""
              onClick={() => setGalleryIndex(i)}
              style={{
                width: 48,
                height: 48,
                objectFit: 'cover',
                borderRadius: 4,
                cursor: 'pointer',
                border: i === galleryIndex ? '2px solid var(--md-sys-color-primary)' : '2px solid transparent',
                opacity: i === galleryIndex ? 1 : 0.6,
                flexShrink: 0,
              }}
              fallback={
                <div style={{
                  width: 48,
                  height: 48,
                  borderRadius: 4,
                  background: 'var(--md-sys-color-surface-container-high)',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  flexShrink: 0,
                }}>
                  <FileText size={16} className="text-secondary" />
                </div>
              }
            />
          ))}
        </div>
      )}

      {/* Fullscreen image viewer */}
      {fullscreenImageIndex !== null && gallery[fullscreenImageIndex] && (
        <div
          style={{
            position: 'fixed',
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            background: 'rgba(0,0,0,0.9)',
            zIndex: 9999,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
          onClick={() => setFullscreenImageIndex(null)}
        >
          <CacheImage
            srcPath={gallery[fullscreenImageIndex]}
            alt="Fullscreen"
            style={{ maxWidth: '90%', maxHeight: '90%', objectFit: 'contain' }}
            onClick={(e) => e.stopPropagation()}
            fallback={
              <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '12px' }}>
                <FileText size={64} className="text-secondary" />
                <span style={{ color: 'var(--md-sys-color-outline)', fontSize: '14px' }}>Failed to load image</span>
              </div>
            }
          />
          <button
            onClick={(e) => {
              e.stopPropagation();
              setFullscreenImageIndex(null);
            }}
            style={{
              position: 'absolute',
              top: 24,
              right: 24,
              background: 'rgba(255,255,255,0.2)',
              border: 'none',
              borderRadius: '50%',
              width: 48,
              height: 48,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              cursor: 'pointer',
              color: '#fff',
            }}
          >
            <X size={24} />
          </button>

          {gallery.length > 1 && (
            <>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  const nextIndex = (fullscreenImageIndex - 1 + gallery.length) % gallery.length;
                  setFullscreenImageIndex(nextIndex);
                  setGalleryIndex(nextIndex);
                }}
                style={{
                  position: 'absolute',
                  left: 24,
                  top: '50%',
                  transform: 'translateY(-50%)',
                  background: 'rgba(255,255,255,0.2)',
                  border: 'none',
                  borderRadius: '50%',
                  width: 64,
                  height: 64,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  cursor: 'pointer',
                  color: '#fff',
                }}
              >
                <ChevronLeft size={32} />
              </button>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  const nextIndex = (fullscreenImageIndex + 1) % gallery.length;
                  setFullscreenImageIndex(nextIndex);
                  setGalleryIndex(nextIndex);
                }}
                style={{
                  position: 'absolute',
                  right: 24,
                  top: '50%',
                  transform: 'translateY(-50%)',
                  background: 'rgba(255,255,255,0.2)',
                  border: 'none',
                  borderRadius: '50%',
                  width: 64,
                  height: 64,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  cursor: 'pointer',
                  color: '#fff',
                }}
              >
                <ChevronRight size={32} />
              </button>
              <div
                style={{
                  position: 'absolute',
                  bottom: 24,
                  left: '50%',
                  transform: 'translateX(-50%)',
                  background: 'rgba(0,0,0,0.6)',
                  borderRadius: 16,
                  padding: '4px 16px',
                  fontSize: '14px',
                  color: '#fff',
                }}
              >
                {fullscreenImageIndex + 1} / {gallery.length}
              </div>
            </>
          )}
        </div>
      )}
    </>
  );
};
