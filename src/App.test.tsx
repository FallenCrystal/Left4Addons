import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import './i18n';
import { setAppLanguage } from './i18n';
import type { CoreEvent, DashboardSnapshot } from './api/ipc';
import { previewEngine } from './api/previewStore';
import { App } from './App';

const invokeMock = vi.fn();
let eventHandler: ((ev: { payload: CoreEvent }) => void) | null = null;

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
  convertFileSrc: (path: string, protocol: string) => `${protocol}://localhost/${path}`,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: (ev: { payload: CoreEvent }) => void) => {
    eventHandler = cb;
    return () => {
      eventHandler = null;
    };
  }),
}));

function setTauriMode(enabled: boolean) {
  const win = window as unknown as Record<string, unknown>;
  if (enabled) {
    win.__TAURI_INTERNALS__ = {};
  } else {
    delete win.__TAURI_INTERNALS__;
  }
}

describe('Left4Addons v2 Material 3 frontend', () => {
  beforeEach(() => {
    setTauriMode(false);
    invokeMock.mockReset();
    eventHandler = null;
    previewEngine.reset();
    setAppLanguage('en');
  });

  afterEach(() => {
    setTauriMode(false);
    cleanup();
  });

  it('renders library items and navigates across all 5 M3 rail destinations', async () => {
    render(<App />);

    expect(await screen.findByText('Chernobyl: Chapter One')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Organize' }));
    expect(await screen.findByRole('tab', { name: /Bundles/ })).toBeTruthy();
    expect(screen.getByRole('tab', { name: /Collections/ })).toBeTruthy();
    expect(screen.getByRole('tab', { name: /Groups/ })).toBeTruthy();
    expect(screen.getByRole('tab', { name: /Profiles/ })).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: /Reconcile/ }));
    expect(await screen.findByText('Dummy Overwritten')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Workshop' }));
    expect(await screen.findByPlaceholderText('Search Workshop')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    expect(await screen.findByText('Loading Directory')).toBeTruthy();
  });

  it('requires confirmation before executing destructive reconcile actions', async () => {
    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');

    fireEvent.click(screen.getByRole('button', { name: /Reconcile/ }));
    const deleteLocalBtn = await screen.findByRole('button', {
      name: 'Delete Local, Keep Workshop',
    });
    fireEvent.click(deleteLocalBtn);

    const dialog = await screen.findByRole('dialog', { name: 'Confirm File Action' });
    expect(within(dialog).getByText('2814104912.vpk')).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete Local, Keep Workshop' }));

    await waitFor(() => {
      expect(screen.queryByText('Dummy Overwritten')).toBeNull();
    });
  });

  it('defaults to HTTPS-only in Sources & SDK and saves the switch value', async () => {
    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByRole('button', { name: 'Sources & SDK' }));

    const httpsOnly = screen.getByRole('switch', { name: 'Require Secure HTTPS Connections' });
    expect(httpsOnly.getAttribute('aria-checked')).toBe('true');
    fireEvent.click(httpsOnly);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(async () => {
      expect((await previewEngine.getDashboardSnapshot()).config.httpsOnly).toBe(false);
    });
  });

  it('loads desktop remote covers through the app protocol', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    const coverUrl = 'http://catalog.example.com/cover.jpg?size=64&v=2';
    snapshot.addons[0].coverImage = coverUrl;
    invokeMock.mockResolvedValue(snapshot);
    render(<App />);

    const img = await screen.findByAltText(snapshot.addons[0].title);
    const src = new URL(img.getAttribute('src')!);
    expect(src.protocol).toBe('l4a-cover:');
    expect(src.searchParams.get('url')).toBe(coverUrl);
  });

  it('excludes hidden filtered-out addons from batch actions', async () => {
    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');

    fireEvent.click(screen.getByRole('checkbox', { name: 'Chernobyl: Chapter One' }));
    expect(screen.getByText('1 selected')).toBeTruthy();

    const searchInput = screen.getByPlaceholderText('Title, author, ID, filename');
    fireEvent.change(searchInput, { target: { value: 'zzzz-no-match' } });

    expect(screen.getByText('0 selected')).toBeTruthy();
    expect(screen.getByText('1 hidden')).toBeTruthy();
    const deleteBtn = screen.getByRole('button', { name: 'Delete' }) as HTMLButtonElement;
    expect(deleteBtn.disabled).toBe(true);
  });

  it('closes only the topmost overlay when Escape is pressed on stacked modals', async () => {
    render(<App />);
    const trigger = await screen.findByRole('button', { name: 'Chernobyl: Chapter One' });
    fireEvent.click(trigger);

    const sheet = await screen.findByRole('dialog', { name: 'Chernobyl: Chapter One' });
    fireEvent.click(within(sheet).getByRole('button', { name: 'Delete' }));

    expect(await screen.findByRole('dialog', { name: 'Delete Addons' })).toBeTruthy();

    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => {
      expect(screen.queryByRole('dialog', { name: 'Delete Addons' })).toBeNull();
    });
    // Underlying detail side sheet remains open.
    expect(screen.getByRole('dialog', { name: 'Chernobyl: Chapter One' })).toBeTruthy();
  });

  it('ignores out-of-order IPC snapshots and displays partial batch failure recovery details', async () => {
    setTauriMode(true);
    const baseSnapshot = await previewEngine.getDashboardSnapshot();
    const staleSnapshot: DashboardSnapshot = {
      ...baseSnapshot,
      addons: [
        {
          ...baseSnapshot.addons[0],
          id: 'ws:stale',
          title: 'Stale Old Addon',
        },
      ],
    };
    const freshSnapshot: DashboardSnapshot = {
      ...baseSnapshot,
      addons: [
        {
          ...baseSnapshot.addons[0],
          id: 'ws:fresh',
          title: 'Fresh Scanned Addon',
        },
      ],
    };

    let resolveInitial!: (v: DashboardSnapshot) => void;
    let initialDone = false;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_dashboard_snapshot') {
        if (!initialDone) {
          initialDone = true;
          return new Promise<DashboardSnapshot>((r) => {
            resolveInitial = r;
          });
        }
        return Promise.resolve(freshSnapshot);
      }
      if (cmd === 'scan_workspace') {
        return Promise.resolve(freshSnapshot);
      }
      if (cmd === 'toggle_addons') {
        return Promise.resolve({
          items: [
            {
              addonId: 'ws:fresh',
              status: 'failed',
              error: { code: 'recovery_required', message: 'Backup kept at /tmp/bak' },
            },
          ],
          refreshError: { code: 'io', message: 'Disk busy' },
        });
      }
      return Promise.resolve(freshSnapshot);
    });

    render(<App />);

    // Event-driven refresh arrives BEFORE delayed initial snapshot resolves.
    await waitFor(() => expect(eventHandler).not.toBeNull());
    eventHandler?.({ payload: { type: 'snapshot_updated', externalChange: true } });
    expect(await screen.findByText('Fresh Scanned Addon')).toBeTruthy();

    // Delayed initial snapshot must NOT overwrite fresh state.
    resolveInitial(staleSnapshot);
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByText('Fresh Scanned Addon')).toBeTruthy();
    expect(screen.queryByText('Stale Old Addon')).toBeNull();

    // Toggle triggers batch report dialog with recovery_required and refreshError.
    fireEvent.click(screen.getByRole('switch', { name: 'Fresh Scanned Addon' }));
    const reportDialog = await screen.findByRole('dialog', { name: 'Batch Result' });
    expect(within(reportDialog).getByText('Manual recovery required')).toBeTruthy();
    expect(within(reportDialog).getByText(/Disk busy/)).toBeTruthy();
  });
});
