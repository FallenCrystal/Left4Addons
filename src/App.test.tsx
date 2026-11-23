import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    minimize: vi.fn(async () => {}),
    toggleMaximize: vi.fn(async () => {}),
    isMaximized: vi.fn(async () => false),
    close: vi.fn(async () => {}),
    startDragging: vi.fn(async () => {}),
    onResized: vi.fn(async () => () => {}),
  }),
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
    expect((await screen.findAllByText('Dummy Overwritten')).length).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole('button', { name: 'Workshop' }));
    expect(await screen.findByPlaceholderText('Search Workshop')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    expect(await screen.findByText('Manual Addons Directory (addons)')).toBeTruthy();
  });

  it('collapses campaign bundles in Library and drills down into member AddonInfo URLs', async () => {
    render(<App />);
    // Chernobyl Assets Pack is collapsed under the Chernobyl: Chapter One campaign bundle card by default
    const trigger = await screen.findByRole('button', { name: 'Chernobyl: Chapter One' });
    expect(screen.queryByRole('button', { name: 'Chernobyl Assets Pack' })).toBeNull();
    fireEvent.click(trigger);

    const bundleSheet = await screen.findByRole('dialog', { name: 'Chernobyl: Chapter One' });
    expect(within(bundleSheet).getByRole('tab', { name: /Members/ })).toBeTruthy();
    expect(within(bundleSheet).getByRole('tab', { name: /Maps/ })).toBeTruthy();

    // Drill down from bundle member #1 into the full AddonDetailSheet
    fireEvent.click(within(bundleSheet).getByText('Chernobyl Assets Pack'));
    const assetSheet = await screen.findByRole('dialog', { name: 'Chernobyl Assets Pack' });
    // Non-map asset pack must NOT render a "Maps 0" tab
    expect(within(assetSheet).queryByRole('tab', { name: /Maps/ })).toBeNull();
    fireEvent.keyDown(window, { key: 'Escape' });

    // Toggle Group Parts off to expand individual parts in Library, then open Chapter One detail sheet
    fireEvent.click(screen.getByRole('button', { name: 'Group Parts' }));
    expect(await screen.findByRole('button', { name: 'Chernobyl Assets Pack' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Chernobyl: Chapter One' }));

    const detailSheet = await screen.findByRole('dialog', { name: 'Chernobyl: Chapter One' });
    expect(within(detailSheet).getByRole('button', { name: 'Show in Explorer' })).toBeTruthy();
    expect(
      within(detailSheet).getByRole('button', {
        name: 'https://steamcommunity.com/sharedfiles/filedetails/?id=2814104912',
      }),
    ).toBeTruthy();
  });

  it('filters by M3 category dropdown, switches component naming preference, hides Maps 0, and copies raw map code without map prefix', async () => {
    const writeTextMock = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText: writeTextMock } });

    render(<App />);
    await screen.findByText('Yama Campaign Remastered');

    // Category filter is an M3 Select combobox instead of a sprawling chip bar
    const categorySelect = screen.getByRole('combobox', { name: 'Category' });
    expect(categorySelect.textContent).toContain('All categories');
    fireEvent.click(categorySelect);
    const categoryMenu = screen.getByRole('listbox', { name: 'Category' });
    fireEvent.click(within(categoryMenu).getByRole('option', { name: /Weapon/ }));
    expect(screen.getByText('Tactical M4A1 SOPMOD')).toBeTruthy();
    expect(screen.queryByText('Yama Campaign Remastered')).toBeNull();

    // Weapon addon has 0 maps -> right-side sheet must NOT show "Maps 0" tab
    fireEvent.click(screen.getByRole('button', { name: 'Tactical M4A1 SOPMOD' }));
    const weaponSheet = await screen.findByRole('dialog', { name: 'Tactical M4A1 SOPMOD' });
    expect(within(weaponSheet).queryByRole('tab')).toBeNull();
    fireEvent.keyDown(window, { key: 'Escape' });

    // Selecting Map category keeps multi-part Chernobyl bundle collapsed as 2 parts (even though Part 2 is Textures/Models)
    fireEvent.click(categorySelect);
    fireEvent.click(
      within(screen.getByRole('listbox', { name: 'Category' })).getByRole('option', {
        name: /Map/,
      }),
    );
    expect(screen.getByRole('button', { name: 'Chernobyl: Chapter One' })).toBeTruthy();
    expect(screen.getByText('2 parts')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Chernobyl Assets Pack' })).toBeNull();

    // Reset category back to All categories
    fireEvent.click(categorySelect);
    fireEvent.click(
      within(screen.getByRole('listbox', { name: 'Category' })).getByRole('option', {
        name: /All categories/,
      }),
    );

    // Campaign addon with maps copies raw map ID without "map " prefix
    fireEvent.click(await screen.findByRole('button', { name: 'Yama Campaign Remastered' }));
    const yamaSheet = await screen.findByRole('dialog', { name: 'Yama Campaign Remastered' });
    fireEvent.click(within(yamaSheet).getByRole('tab', { name: /Maps/ }));
    const copyMapBtn = within(yamaSheet).getByRole('button', { name: 'Copy map ID l4d_yama_1' });
    expect(copyMapBtn.textContent).not.toContain('map l4d_yama_1');
    fireEvent.click(copyMapBtn);
    await waitFor(() => {
      expect(writeTextMock).toHaveBeenCalledWith('l4d_yama_1');
    });
    fireEvent.keyDown(window, { key: 'Escape' });

    // Switch Component Name preference to Filename in Settings -> Appearance
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByRole('button', { name: 'Appearance' }));
    const nameGroup = screen.getByRole('group', { name: 'Component Name' });
    fireEvent.click(within(nameGroup).getByRole('button', { name: 'Filename' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    expect(await screen.findByText('[3011942811] Yama_Campaign_Remastered')).toBeTruthy();
    expect(screen.getByText('local_custom_soundboard')).toBeTruthy();
  });

  it('supports batch reconcile, help guide dialog, and inline organize inspector', async () => {
    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');

    // Manual scope label replaces misleading Loading tag
    expect(screen.getByRole('button', { name: /Manual/ })).toBeTruthy();

    // Organize inline inspector opens when clicking a group card
    fireEvent.click(screen.getByRole('button', { name: 'Organize' }));
    fireEvent.click(await screen.findByRole('tab', { name: /Groups/ }));
    fireEvent.click(await screen.findByText('Campaigns'));
    const groupSheet = await screen.findByRole('dialog', { name: 'Campaigns' });
    expect(within(groupSheet).getByText('Chernobyl: Chapter One')).toBeTruthy();
    fireEvent.keyDown(window, { key: 'Escape' });

    // Reconcile help dialog and batch recommended action
    fireEvent.click(screen.getByRole('button', { name: /Reconcile/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Reconcile & Action Effects Guide' }));
    expect(await screen.findByRole('dialog', { name: 'Reconcile & Action Effects Guide' })).toBeTruthy();
    fireEvent.keyDown(window, { key: 'Escape' });

    fireEvent.click(screen.getByRole('button', { name: /Apply Recommended/ }));
    const batchDialog = await screen.findByRole('dialog', { name: 'Confirm Batch Reconcile' });
    fireEvent.click(within(batchDialog).getByRole('button', { name: /Confirm/ }));

    await waitFor(() => {
      expect(screen.getByText('All synced')).toBeTruthy();
    });
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
    snapshot.addons[2].coverImage = coverUrl;
    invokeMock.mockResolvedValue(snapshot);
    render(<App />);

    const img = await screen.findByAltText(snapshot.addons[2].title);
    const src = new URL(img.getAttribute('src')!);
    expect(src.protocol).toBe('l4a-cover:');
    expect(src.searchParams.get('url')).toBe(coverUrl);
  });

  it('excludes hidden filtered-out addons from batch actions', async () => {
    render(<App />);
    await screen.findByText('Yama Campaign Remastered');

    // Selecting standalone addon selects 1 item; selecting 2-part collapsed bundle adds 2 items
    fireEvent.click(screen.getByRole('checkbox', { name: 'Yama Campaign Remastered' }));
    expect(screen.getByText('1 selected')).toBeTruthy();

    fireEvent.click(screen.getByRole('checkbox', { name: 'Chernobyl: Chapter One' }));
    expect(screen.getByText('3 selected')).toBeTruthy();

    const searchInput = screen.getByPlaceholderText('Title, author, ID, filename');
    fireEvent.change(searchInput, { target: { value: 'zzzz-no-match' } });

    expect(screen.getByText('0 selected')).toBeTruthy();
    expect(screen.getByText('3 hidden')).toBeTruthy();
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

  it('uses custom M3 select menu and M3 tooltip attributes instead of native browser popups', async () => {
    const { container } = render(<App />);
    await screen.findByText('Chernobyl: Chapter One');

    expect(container.querySelectorAll('select').length).toBe(0);
    expect(container.querySelectorAll('[title]').length).toBe(0);

    const sortTrigger = screen.getByRole('combobox', { name: 'Sort' });
    expect(sortTrigger.getAttribute('aria-expanded')).toBe('false');

    fireEvent.click(sortTrigger);
    expect(sortTrigger.getAttribute('aria-expanded')).toBe('true');

    const listbox = screen.getByRole('listbox', { name: 'Sort' });
    const sizeOption = within(listbox).getByRole('option', { name: 'Size' });
    fireEvent.click(sizeOption);

    expect(sortTrigger.getAttribute('aria-expanded')).toBe('false');
    expect(sortTrigger.textContent).toContain('Size');
  });

  it('shows queued Workshop downloads without a speed and keeps failure details readable', async () => {
    setTauriMode(true);
    let snapshot: DashboardSnapshot = { ...(await previewEngine.getDashboardSnapshot()), tasks: [] };
    const item = {
      ...(await previewEngine.fetchWorkshopItem('3350912881')),
      workshopId: '55443321',
      title: 'Uncached Workshop addon',
    };
    const task = {
      id: 'queued-workshop-download',
      kind: 'download' as const,
      status: 'queued' as const,
      targetId: item.workshopId,
      targetTitle: item.title,
      progressPercent: 0,
      createdAt: '2026-11-22T10:00:00Z',
      updatedAt: '2026-11-22T10:00:00Z',
    };
    let rejectDownload!: (reason: { code: string; message: string }) => void;
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'search_workshop') {
        return { items: [item], total: 1, page: 1, pageSize: 24, source: item.source };
      }
      if (command === 'download_workshop_item') {
        return new Promise((_resolve, reject) => {
          rejectDownload = reject;
        });
      }
      return snapshot;
    });

    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');
    await waitFor(() => expect(eventHandler).not.toBeNull());
    fireEvent.click(screen.getByRole('button', { name: 'Workshop' }));
    const card = (await screen.findByText(item.title)).closest('article')!;
    fireEvent.click(within(card).getByRole('button', { name: 'Download' }));
    expect(invokeMock).toHaveBeenCalledWith('download_workshop_item', {
      workshopId: item.workshopId,
      titleHint: item.title,
    });

    fireEvent.click(screen.getByRole('button', { name: 'Tasks' }));
    const sheet = within(await screen.findByRole('dialog', { name: 'Tasks' }));
    snapshot = { ...snapshot, tasks: [task] };
    act(() => eventHandler?.({ payload: { type: 'snapshot_updated', externalChange: false } }));
    expect(await sheet.findByText('Queued')).toBeTruthy();
    expect(sheet.getByText(item.title)).toBeTruthy();
    expect(sheet.queryByText('0 B/s')).toBeNull();

    snapshot = { ...snapshot, tasks: [{ ...task, status: 'running' }] };
    act(() => eventHandler?.({ payload: { type: 'snapshot_updated', externalChange: false } }));
    expect(await sheet.findByText('0%')).toBeTruthy();
    expect(sheet.queryByText('Queued')).toBeNull();

    const error = 'Remote download returned HTTP status 403: Request rejected by the mirror.';
    snapshot = { ...snapshot, tasks: [{ ...task, status: 'failed', error }] };
    act(() => rejectDownload({ code: 'network', message: error }));
    expect(await sheet.findByText('Failed')).toBeTruthy();
    expect(sheet.getByText(item.title)).toBeTruthy();
    expect(sheet.getByText(error).style.overflowWrap).toBe('anywhere');
  });

  it('displays addon titles in the task queue and refreshes download speed every second', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.tasks = [
      {
        id: 'task-dl-history',
        kind: 'download',
        status: 'completed',
        targetId: '99887766',
        targetTitle: 'Dead Air Redux',
        progressPercent: 100,
        message: 'Done',
        createdAt: '2026-11-22T10:00:00Z',
        updatedAt: '2026-11-22T10:01:00Z',
      },
    ];
    invokeMock.mockResolvedValue(snapshot);

    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');
    await waitFor(() => expect(eventHandler).not.toBeNull());

    fireEvent.click(screen.getByRole('button', { name: 'Tasks' }));
    const sheet = await screen.findByRole('dialog', { name: 'Tasks' });
    expect(within(sheet).getByText('Dead Air Redux')).toBeTruthy();
    expect(within(sheet).getByText(/#99887766/)).toBeTruthy();

    vi.useFakeTimers();
    try {
      // Simulate rapid chunks (<50ms apart) without backend speedBps — speed should still refresh every 1s.
      act(() => {
        eventHandler?.({
          payload: {
            type: 'download_progress',
            taskId: 'dl-1',
            workshopId: '55443322',
            title: 'Cold Stream Overhaul',
            downloaded: 0,
            total: 10 * 1024 * 1024,
            percent: 0,
          },
        });
      });

      expect(within(sheet).getByText('Cold Stream Overhaul')).toBeTruthy();
      expect(within(sheet).getByText('#55443322')).toBeTruthy();
      expect(within(sheet).getByText('0 B/s')).toBeTruthy();

      for (let i = 1; i <= 4; i += 1) {
        act(() => {
          vi.advanceTimersByTime(10);
          eventHandler?.({
            payload: {
              type: 'download_progress',
              taskId: 'dl-1',
              workshopId: '55443322',
              title: 'Cold Stream Overhaul',
              downloaded: i * 512 * 1024,
              total: 10 * 1024 * 1024,
              percent: i * 5,
            },
          });
        });
      }

      // Before the 1-second tick, speed has not flickered yet.
      expect(within(sheet).getByText('0 B/s')).toBeTruthy();

      // Advance to the 1-second speed refresh tick (2 MB downloaded in 1s -> 2.0 MB/s).
      act(() => {
        vi.advanceTimersByTime(960);
      });
      expect(within(sheet).getByText('2.0 MB/s')).toBeTruthy();

      // Stall for 1 second with no new bytes -> speed refreshes back to 0 B/s.
      act(() => {
        vi.advanceTimersByTime(1000);
      });
      expect(within(sheet).getByText('0 B/s')).toBeTruthy();

      // Backend-authoritative speedBps updates cleanly and drops to 0 B/s if events stop > 1.5s.
      act(() => {
        eventHandler?.({
          payload: {
            type: 'download_progress',
            taskId: 'dl-1',
            workshopId: '55443322',
            title: 'Cold Stream Overhaul',
            downloaded: 6 * 1024 * 1024,
            total: 10 * 1024 * 1024,
            percent: 60,
            speedBps: 4 * 1024 * 1024,
          },
        });
      });
      expect(within(sheet).getByText('4.0 MB/s')).toBeTruthy();

      act(() => {
        vi.advanceTimersByTime(2000);
      });
      expect(within(sheet).getByText('0 B/s')).toBeTruthy();

      act(() => {
        eventHandler?.({
          payload: {
            type: 'download_progress',
            taskId: 'dl-1',
            workshopId: '55443322',
            downloaded: 12 * 1024 * 1024,
            total: null,
            percent: 0,
          },
        });
      });
      expect(within(sheet).getByText('12.0 MB')).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });
});
