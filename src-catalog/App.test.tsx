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
