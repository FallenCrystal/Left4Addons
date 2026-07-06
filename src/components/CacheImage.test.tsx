import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { CacheImage } from './CacheImage';

describe('CacheImage', () => {
  test('renders image with mapped URL', () => {
    render(
      <CacheImage
        srcPath="/cache/my_addon_img.jpg"
        alt="Addon Image"
        fallback={<span>Fallback content</span>}
      />
    );

    const img = screen.getByAltText('Addon Image') as HTMLImageElement;
    expect(img).toBeDefined();
    // getImageUrl transforms "/cache/my_addon_img.jpg" to "http://cache.localhost/my_addon_img.jpg" (on non-Apple systems)
    expect(img.src).toContain('my_addon_img.jpg');
  });

  test('renders fallback when srcPath is empty', () => {
    render(
      <CacheImage
        srcPath=""
        alt="Addon Image"
        fallback={<span>Fallback content</span>}
      />
    );

    expect(screen.queryByAltText('Addon Image')).toBeNull();
    expect(screen.getByText('Fallback content')).toBeDefined();
  });

  test('renders fallback and triggers onError callback when image loading fails', () => {
    const onErrorMock = vi.fn();
    render(
      <CacheImage
        srcPath="/cache/failing_img.jpg"
        alt="Addon Image"
        fallback={<span>Fallback content</span>}
        onError={onErrorMock}
      />
    );

    const img = screen.getByAltText('Addon Image');
    expect(img).toBeDefined();

    // Trigger error event
    fireEvent.error(img);

    expect(onErrorMock).toHaveBeenCalledTimes(1);
    expect(screen.queryByAltText('Addon Image')).toBeNull();
    expect(screen.getByText('Fallback content')).toBeDefined();
  });
});
