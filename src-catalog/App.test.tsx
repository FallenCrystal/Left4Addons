import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';

vi.mock('./components/OverviewPanel', () => ({
  OverviewPanel: ({ onInspectItem }: { onInspectItem: (id: string) => void }) => (
    <button onClick={() => onInspectItem('4201')}>Inspect item</button>
  ),
}));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it('keeps the override draft and audit reason when the overview polls', async () => {
  vi.useFakeTimers();
  const detail = {
    item: {
      workshopId: '4201',
      status: 'active',
      title: 'Saved title',
      tags: [],
      dependencies: [],
      collectionChildren: [],
    },
    revisions: [],
  };
  const fetchMock = vi.fn(async (path: string) => {
    if (path.endsWith('/auth/me')) {
      return Response.json({ authenticated: true, username: 'admin' });
    }
    if (path.endsWith('/overview')) {
      return Response.json({ steam: { connectionState: 'connected', mode: 'anonymous' } });
    }
    if (path.endsWith('/items/4201')) {
      return Response.json(detail);
    }
    throw new Error(`Unexpected request ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);

  await act(async () => {
    render(<App />);
  });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Inspect item' }));
  });
  const reason = screen.getByPlaceholderText<HTMLInputElement>('如：上游误删，锁定历史元数据与依赖边');
  fireEvent.change(reason, { target: { value: 'Draft audit reason' } });
  fireEvent.change(screen.getByDisplayValue('Saved title'), { target: { value: 'Draft title' } });

  await act(async () => {
    await vi.advanceTimersByTimeAsync(8000);
  });

  expect(reason.value).toBe('Draft audit reason');
  expect(screen.getByDisplayValue('Draft title')).toBeTruthy();
  const count = (suffix: string) =>
    fetchMock.mock.calls.filter(([path]) => path.endsWith(suffix)).length;
  expect(count('/overview')).toBe(2);
  expect(count('/items/4201')).toBe(1);
});

it('keeps the console open on a failed logout and lets the admin retry', async () => {
  let logoutAttempts = 0;
  const fetchMock = vi.fn(async (path: string, init?: RequestInit) => {
    if (path.endsWith('/auth/me')) {
      return Response.json({ authenticated: true, username: 'admin', csrfToken: 'fixture-csrf' });
    }
    if (path.endsWith('/overview')) {
      return Response.json({ steam: { connectionState: 'connected', mode: 'anonymous' } });
    }
    if (path.endsWith('/auth/logout')) {
      expect(new Headers(init?.headers).get('X-L4A-CSRF')).toBe('fixture-csrf');
      if (++logoutAttempts === 1) throw new Error('Offline');
      return Response.json({ ok: true });
    }
    throw new Error(`Unexpected request ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);

  await act(async () => {
    render(<App />);
  });
  const logout = screen.getByRole('button', { name: '退出 (admin)' });
  await act(async () => fireEvent.click(logout));
  expect(screen.getByRole('status').textContent).toContain('退出未完成，请重试');
  expect(screen.getByRole('button', { name: '退出 (admin)' })).toBeTruthy();

  await act(async () => fireEvent.click(logout));
  expect(logoutAttempts).toBe(2);
  expect(screen.queryByRole('button', { name: '退出 (admin)' })).toBeNull();
});

it('reloads overrides on unlock and can explicitly clear a cached download URL', async () => {
  const oldUrl = 'https://example.test/old.vpk';
  const newUrl = 'https://example.test/new.vpk';
  const locked = {
    item: {
      workshopId: '4201',
      status: 'active',
      title: 'Override',
      tags: [],
      dependencies: [{ workshopId: '4202', required: true }],
      collectionChildren: [],
      fileUrl: oldUrl,
    },
    isAdminOverride: true,
    revisions: [],
  };
  const unlocked = {
    ...locked,
    isAdminOverride: false,
    item: {
      ...locked.item,
      title: 'Fresh title',
      fileUrl: newUrl,
      dependencies: [{ workshopId: '4203', required: true }],
    },
  };
  let overridePayload: unknown;
  const fetchMock = vi.fn(async (path: string, init?: RequestInit) => {
    if (path.endsWith('/auth/me')) return Response.json({ authenticated: true, username: 'admin' });
    if (path.endsWith('/overview')) {
      return Response.json({ steam: { connectionState: 'connected', mode: 'anonymous' } });
    }
    if (path.endsWith('/items/4201/override')) {
      if (init?.method === 'DELETE') return Response.json(unlocked);
      if (typeof init?.body === 'string') {
        overridePayload = JSON.parse(init.body);
        return Response.json(unlocked.item);
      }
    }
    if (path.endsWith('/items/4201')) return Response.json(locked);
    throw new Error(`Unexpected request ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);

  await act(async () => {
    render(<App />);
  });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Inspect item' })));
  expect(screen.getByDisplayValue(oldUrl)).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: '解除覆写锁定' })));

  expect(screen.getByDisplayValue('Fresh title')).toBeTruthy();
  fireEvent.change(screen.getByDisplayValue(newUrl), { target: { value: '' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: '保存并锁定覆写' })));
  expect(overridePayload).toMatchObject({
    fileUrl: '',
    dependencies: [{ workshopId: '4203', required: true }],
  });
});

it('configures the content filter model and runs the probability test sandbox', async () => {
  let savedConfig: unknown;
  let sandboxRequest: unknown;
  const fetchMock = vi.fn(async (path: string, init?: RequestInit) => {
    if (path.endsWith('/auth/me')) {
      return Response.json({ authenticated: true, username: 'admin', csrfToken: 'csrf-tok' });
    }
    if (path.endsWith('/overview')) {
      return Response.json({
        steam: { connectionState: 'connected', mode: 'anonymous' },
        filter: {
          config: {
            enabled: false,
            backend: 'managed_python',
            cacheTtlSecs: 3600,
            params: {
              threads: 4,
              enableText: true,
              enableImage: true,
              maxImages: 2,
              galleryAggregation: 'max',
              goreWeight: 1.0,
              nsfwWeight: 1.0,
              temperature: 1.0,
              bias: 0.0,
            },
          },
          ready: false,
          totalRequests: 12,
          cacheHits: 6,
          failedRequests: 0,
        },
      });
    }
    if (path.endsWith('/admin/config/filter') && init?.method === 'PATCH') {
      savedConfig = JSON.parse(String(init.body));
      return Response.json({
        config: savedConfig,
        ready: true,
        activeEndpoint: 'http://127.0.0.1:8090',
        modelName: 'Left4Filters',
        modelVersion: '2.1.0',
        totalRequests: 12,
        cacheHits: 6,
        failedRequests: 0,
      });
    }
    if (path.endsWith('/admin/filter/test') && init?.method === 'POST') {
      sandboxRequest = JSON.parse(String(init.body));
      return Response.json({
        prob: 0.9125,
        goreProb: 0.0,
        nsfwProb: 0.9125,
        textProb: 0.88,
        titleProb: 0.91,
        descriptionProb: 0.42,
        imageProb: 0.76,
      });
    }
    throw new Error(`Unexpected request ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);

  await act(async () => {
    render(<App />);
  });

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '内容过滤模型' }));
  });

  fireEvent.click(screen.getByLabelText('启用内容过滤概率接口'));
  fireEvent.change(screen.getByLabelText('推理后端模式'), {
    target: { value: 'upstream_http' },
  });
  fireEvent.change(screen.getByLabelText('外部推理服务 URL (upstream_url)'), {
    target: { value: 'http://127.0.0.1:8090' },
  });
  fireEvent.click(screen.getByRole('button', { name: '仅擦边 NSFW' }));

  expect(screen.getByLabelText<HTMLInputElement>('推理线程数 (1-16)').disabled).toBe(true);
  expect(screen.getByText('线程数由外部推理进程自行控制')).toBeTruthy();

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '保存配置' }));
  });

  expect(savedConfig).toMatchObject({
    enabled: true,
    backend: 'upstream_http',
    upstreamUrl: 'http://127.0.0.1:8090',
    params: {
      goreWeight: 0,
      nsfwWeight: 1,
    },
  });
  expect(screen.getByText('就绪 (Ready)')).toBeTruthy();

  fireEvent.change(screen.getByLabelText('Workshop ID 或工坊链接 (可选)'), {
    target: { value: 'https://steamcommunity.com/sharedfiles/filedetails/?id=2814104912' },
  });
  fireEvent.change(screen.getByLabelText('自定义标题 (可选)'), {
    target: { value: 'Sample Skin' },
  });

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '执行概率试算' }));
  });

  expect(sandboxRequest).toEqual({
    workshopId: '2814104912',
    title: 'Sample Skin',
  });
  expect(screen.getAllByText('91.25% (0.9125)').length).toBeGreaterThanOrEqual(1);
});

it('preserves filter draft and sandbox inputs across tab switches, supports natural decimal/negative typing, and guards against stale overview polls', async () => {
  vi.useFakeTimers();
  let resolveStaleOverview: ((res: Response) => void) | null = null;
  let overviewCalls = 0;
  let savedConfig: unknown;
  let sandboxRequest: unknown;

  const initialOverview = {
    steam: { connectionState: 'connected', mode: 'anonymous' },
    filter: {
      config: {
        enabled: true,
        backend: 'managed_python',
        deployDir: '/opt/Left4Filters',
        cacheTtlSecs: 3600,
        params: {
          threads: 4,
          enableText: true,
          enableImage: true,
          maxImages: 2,
          galleryAggregation: 'max',
          goreWeight: 1.0,
          nsfwWeight: 1.0,
          temperature: 1.0,
          bias: 0.0,
        },
      },
      ready: true,
      runningPid: 4321,
      activeEndpoint: 'http://127.0.0.1:19001',
      modelName: 'Left4Filters',
      modelVersion: '2.2.0',
      totalRequests: 5,
      cacheHits: 2,
      failedRequests: 0,
    },
  };

  const fetchMock = vi.fn((path: string, init?: RequestInit) => {
    if (path.endsWith('/auth/me')) {
      return Promise.resolve(
        Response.json({ authenticated: true, username: 'admin', csrfToken: 'csrf-2' }),
      );
    }
    if (path.endsWith('/overview')) {
      overviewCalls += 1;
      if (overviewCalls === 2) {
        return new Promise<Response>((resolve) => {
          resolveStaleOverview = resolve;
        });
      }
      return Promise.resolve(Response.json(initialOverview));
    }
    if (path.endsWith('/items/4201')) {
      return Promise.resolve(
        Response.json({
          item: {
            workshopId: '4201',
            status: 'active',
            title: 'Inspected Addon',
            tags: [],
            dependencies: [],
            collectionChildren: [],
          },
          revisions: [],
        }),
      );
    }
    if (path.endsWith('/admin/config/filter') && init?.method === 'PATCH') {
      savedConfig = JSON.parse(String(init.body));
      return Promise.resolve(
        Response.json({
          ...initialOverview.filter,
          config: savedConfig,
          modelVersion: '2.3.0-fresh',
        }),
      );
    }
    if (path.endsWith('/admin/filter/test') && init?.method === 'POST') {
      sandboxRequest = JSON.parse(String(init.body));
      return Promise.resolve(
        Response.json({
          prob: 0.42,
          goreProb: 0.1,
          nsfwProb: 0.4,
          corpseProb: 0.5,
          corpsePileProb: 0.21,
          corpseReviewFloor: 0.2,
        }),
      );
    }
    throw new Error(`Unexpected request ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);

  await act(async () => {
    render(<App />);
  });

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '内容过滤模型' }));
  });

  const tempInput = screen.getByLabelText<HTMLInputElement>('校准温度 temperature (0.25-4)');
  const biasInput = screen.getByLabelText<HTMLInputElement>('对数偏置 bias (-5 ~ +5)');
  const goreInput = screen.getByLabelText<HTMLInputElement>('血腥权重 goreWeight (0-3)');

  // Natural intermediate keystrokes ('', '0', '0.', '0.5') do not eagerly clamp to 0.25.
  fireEvent.focus(tempInput);
  fireEvent.change(tempInput, { target: { value: '' } });
  expect(tempInput.value).toBe('');
  fireEvent.change(tempInput, { target: { value: '0' } });
  expect(tempInput.value).toBe('0');
  fireEvent.change(tempInput, { target: { value: '0.' } });
  expect(tempInput.value).toBe('0.');
  fireEvent.change(tempInput, { target: { value: '0.5' } });
  expect(tempInput.value).toBe('0.5');
  fireEvent.blur(tempInput);
  expect(tempInput.value).toBe('0.5');

  // Natural negative typing ('-', '-0', '-0.', '-0.2') works without clamping mid-stroke.
  fireEvent.focus(biasInput);
  fireEvent.change(biasInput, { target: { value: '-' } });
  expect(biasInput.value).toBe('-');
  fireEvent.change(biasInput, { target: { value: '-0' } });
  expect(biasInput.value).toBe('-0');
  fireEvent.change(biasInput, { target: { value: '-0.' } });
  expect(biasInput.value).toBe('-0.');
  fireEvent.change(biasInput, { target: { value: '-0.2' } });
  expect(biasInput.value).toBe('-0.2');
  fireEvent.blur(biasInput);
  expect(biasInput.value).toBe('-0.2');

  // Out-of-range input clamps cleanly on blur, and empty blur reverts to last valid value.
  fireEvent.focus(goreInput);
  fireEvent.change(goreInput, { target: { value: '9.5' } });
  expect(goreInput.value).toBe('9.5');
  fireEvent.blur(goreInput);
  expect(goreInput.value).toBe('3');

  fireEvent.focus(goreInput);
  fireEvent.change(goreInput, { target: { value: '' } });
  fireEvent.blur(goreInput);
  expect(goreInput.value).toBe('3');

  const imgUrlsTextarea = screen.getByLabelText<HTMLTextAreaElement>(/Steam CDN 预览图链接/);
  expect(imgUrlsTextarea.tagName).toBe('TEXTAREA');
  fireEvent.change(imgUrlsTextarea, {
    target: {
      value:
        'https://images.steamusercontent.com/ugc/1/a.jpg https://cloud-3.steamusercontent.com/ugc/2/b.jpg,\nhttps://media.steampowered.com/ugc/3/c.png',
    },
  });

  // Switch away to Overview tab to inspect an item, then switch back: draft + sandbox stay intact.
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '总览与同步引擎' }));
  });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Inspect item' }));
  });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '内容过滤模型' }));
  });

  expect(tempInput.value).toBe('0.5');
  expect(biasInput.value).toBe('-0.2');
  expect(goreInput.value).toBe('3');
  expect(screen.getByRole('button', { name: '按当前已存配置试算' })).toBeTruthy();
  expect(screen.getByRole('button', { name: '保存并试算' })).toBeTruthy();

  // Trigger the 8s background overview poll (remains in-flight).
  await act(async () => {
    await vi.advanceTimersByTimeAsync(8000);
  });
  expect(resolveStaleOverview).not.toBeNull();

  // Save & test while the older overview poll is still in flight.
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '保存并试算' }));
  });

  expect(savedConfig).toMatchObject({
    params: {
      temperature: 0.5,
      bias: -0.2,
      goreWeight: 3,
    },
  });
  expect(sandboxRequest).toEqual({
    imageUrls: [
      'https://images.steamusercontent.com/ugc/1/a.jpg',
      'https://cloud-3.steamusercontent.com/ugc/2/b.jpg',
      'https://media.steampowered.com/ugc/3/c.png',
    ],
  });
  expect(screen.getByText('Left4Filters (2.3.0-fresh)')).toBeTruthy();
  expect(screen.getByText('可见尸体概率 (corpseProb)')).toBeTruthy();
  expect(screen.getByText('尸堆概率 (corpsePileProb)')).toBeTruthy();
  expect(screen.getByText('尸体复核下限 (corpseReviewFloor)')).toBeTruthy();

  // Now resolve the stale overview poll: overviewEpochRef must discard it!
  await act(async () => {
    resolveStaleOverview?.(Response.json(initialOverview));
  });
  expect(screen.getByText('Left4Filters (2.3.0-fresh)')).toBeTruthy();
  expect(tempInput.value).toBe('0.5');
  expect(biasInput.value).toBe('-0.2');

  // Malformed partial numeric input ("2abc") must revert on blur instead of committing 2.
  fireEvent.focus(goreInput);
  fireEvent.change(goreInput, { target: { value: '2abc' } });
  fireEvent.blur(goreInput);
  expect(goreInput.value).toBe('3');
});

it('preserves FilterPanel draft edits made while PATCH /admin/config/filter is in flight', async () => {
  let resolveSave: ((value: Response) => void) | null = null;
  const baseOverview = {
    steam: { connectionState: 'connected', mode: 'anonymous' },
    catalog: {
      totalItems: 0,
      activeItems: 0,
      removedItems: 0,
      privateItems: 0,
      overrideCount: 0,
      lastFullSyncAt: null,
      lastIncrementalSyncAt: null,
    },
    sync: {
      running: false,
      mode: null,
      startedAt: null,
      finishedAt: null,
      scanned: 0,
      updated: 0,
      removed: 0,
      lastError: null,
    },
    filter: {
      config: {
        enabled: true,
        backend: 'upstream_http',
        upstreamUrl: 'http://127.0.0.1:18787',
        cacheTtlSecs: 300,
        params: {
          threads: 4,
          enableText: true,
          enableImage: true,
          maxImages: 2,
          galleryAggregation: 'max',
          goreWeight: 1,
          nsfwWeight: 1,
          temperature: 1,
          bias: 0,
        },
      },
      ready: true,
      modelName: 'Left4Filters',
      modelVersion: '2.0.0',
      totalRequests: 0,
      cacheHits: 0,
      failedRequests: 0,
    },
  };

  vi.stubGlobal(
    'fetch',
    vi.fn((path: string, init?: RequestInit) => {
      if (path.endsWith('/auth/me')) {
        return Promise.resolve(
          Response.json({ authenticated: true, username: 'admin', csrfToken: 'csrf-3' }),
        );
      }
      if (path.endsWith('/overview')) {
        return Promise.resolve(Response.json(baseOverview));
      }
      if (path.endsWith('/admin/config/filter') && init?.method === 'PATCH') {
        const body = JSON.parse(String(init.body));
        return new Promise<Response>((resolve) => {
          resolveSave = () =>
            resolve(Response.json({ ...baseOverview.filter, config: body }));
        });
      }
      throw new Error(`Unexpected request ${path}`);
    }),
  );

  await act(async () => {
    render(<App />);
  });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: '内容过滤模型' }));
  });

  const goreInput = screen.getByLabelText<HTMLInputElement>('血腥权重 goreWeight (0-3)');
  fireEvent.focus(goreInput);
  fireEvent.change(goreInput, { target: { value: '1.5' } });
  fireEvent.blur(goreInput);

  // Start saving 1.5 (remains pending)
  fireEvent.click(screen.getByRole('button', { name: '保存配置' }));
  expect(resolveSave).not.toBeNull();

  // Edit goreWeight to 2.2 while the first save is still in flight
  fireEvent.focus(goreInput);
  fireEvent.change(goreInput, { target: { value: '2.2' } });
  fireEvent.blur(goreInput);
  expect(goreInput.value).toBe('2.2');

  // Resolve the in-flight save: the concurrent edit (2.2) MUST NOT be discarded!
  await act(async () => {
    resolveSave?.(Response.json(baseOverview.filter));
  });
  expect(goreInput.value).toBe('2.2');
});

