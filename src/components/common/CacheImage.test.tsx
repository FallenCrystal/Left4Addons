import { beforeEach, describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { CacheImage } from './CacheImage';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => [255, 216, 255, 217]),
}));

const mockInvoke = vi.mocked(invoke);

URL.createObjectURL = vi.fn(() => 'blob:mock-image-url');
URL.revokeObjectURL = vi.fn();

describe('CacheImage', () => {
  beforeEach(() => {
    mockInvoke.mockClear();
    mockInvoke.mockImplementation(async () => [255, 216, 255, 217]);
    vi.mocked(URL.createObjectURL).mockClear();
    vi.mocked(URL.revokeObjectURL).mockClear();
  });

  test('loads cached image through IPC', async () => {
    vi.mocked(URL.createObjectURL).mockReturnValueOnce('blob:cache-image-url');

    render(
      <CacheImage
        srcPath="/cache/my_addon_img.jpg"
        alt="Addon Image"
        fallback={<span>Fallback content</span>}
      />
    );

    const img = await screen.findByAltText('Addon Image') as HTMLImageElement;
    expect(img).toBeDefined();
    expect(mockInvoke).toHaveBeenCalledWith('get_cache_image', { imagePath: '/cache/my_addon_img.jpg' });
    expect(img.src).toBe('blob:cache-image-url');
  });

  test('renders remote image directly by default', async () => {
    render(
      <CacheImage
        srcPath="https://example.com/image.jpg"
        alt="Remote Image"
        fallback={<span>Fallback content</span>}
      />
    );

    const img = await screen.findByAltText('Remote Image') as HTMLImageElement;
    expect(img.src).toBe('https://example.com/image.jpg');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  test('caches remote image before rendering when cacheRemote is enabled', async () => {
    vi.mocked(URL.createObjectURL).mockReturnValueOnce('blob:remote-cache-image-url');
    mockInvoke.mockImplementation(async (cmd) => {
      if (cmd === 'cache_remote_image') return '/cache/remote.jpg';
      if (cmd === 'get_cache_image') return [255, 216, 255, 217];
      throw new Error(`Unexpected command: ${cmd}`);
    });

    render(
      <CacheImage
        srcPath="https://example.com/image.jpg"
        cacheRemote
        alt="Remote Image"
        fallback={<span>Fallback content</span>}
      />
    );

    const img = await screen.findByAltText('Remote Image') as HTMLImageElement;
    expect(img.src).toBe('blob:remote-cache-image-url');
    expect(mockInvoke).toHaveBeenCalledWith('cache_remote_image', { url: 'https://example.com/image.jpg' });
    expect(mockInvoke).toHaveBeenCalledWith('get_cache_image', { imagePath: '/cache/remote.jpg' });
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

  test('renders fallback and triggers onError callback when image loading fails', async () => {
    const onErrorMock = vi.fn();
    vi.mocked(URL.createObjectURL).mockReturnValueOnce('blob:failing-image-url');

    render(
      <CacheImage
        srcPath="/cache/failing_img.jpg"
        alt="Addon Image"
        fallback={<span>Fallback content</span>}
        onError={onErrorMock}
      />
    );

    const img = await screen.findByAltText('Addon Image');
    expect(img).toBeDefined();

    // Trigger error event
    fireEvent.error(img);

    expect(onErrorMock).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByAltText('Addon Image')).toBeNull());
    expect(screen.getByText('Fallback content')).toBeDefined();
  });
});
