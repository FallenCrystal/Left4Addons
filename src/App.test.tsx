import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import './i18n';
import { setAppLanguage } from './i18n';
import type {
  AppConfig,
  CoreEvent,
  DashboardSnapshot,
  FilterEvaluationTarget,
} from './api/ipc';
import { previewEngine } from './api/previewStore';
import { App } from './App';
import { buildAddonRevisionHint, computeFilterPolicySignature } from './utils/contentFilter';

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

  it.each(['grid', 'list'])('limits the %s bundle switch to visible manual parts', async (layout) => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    const bundle = snapshot.bundles[0];
    const [chapter, assets] = snapshot.addons;
    chapter.primaryDir = 'workshop';
    assets.primaryDir = 'workshop';
    const [visiblePart, hiddenPart] = [
      { ...assets, id: 'hash:visible', title: 'Local chapters', enabled: true },
      { ...assets, id: 'hash:hidden', title: 'Local textures', enabled: false },
    ].map((part) => ({
      ...part,
      primaryDir: 'loading' as const,
      workshopId: undefined,
      workshopTitle: undefined,
      addonTitle: part.title,
    }));
    bundle.memberAddonIds.push(visiblePart.id, hiddenPart.id);
    snapshot.addons = [chapter, assets, visiblePart, hiddenPart];
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd === 'toggle_addons' ? { items: [] } : snapshot,
    );
    render(<App />);
    await screen.findByText(bundle.name);
    if (layout === 'list') {
      fireEvent.click(screen.getByRole('button', { name: 'List' }));
    }

    const scope = screen.getByRole('group', { name: 'Scope' });
    expect(screen.getByRole('switch', { name: bundle.name }).getAttribute('aria-checked')).toBe(
      'false',
    );
    fireEvent.click(within(scope).getByRole('button', { name: /^Workshop/ }));
    expect(screen.getByText(bundle.name)).toBeTruthy();
    expect(screen.queryByRole('switch', { name: bundle.name })).toBeNull();
    fireEvent.click(screen.getByText(bundle.name));
    const inspector = screen.getByRole('dialog', { name: bundle.name });
    expect(within(inspector).getByRole('switch', { name: 'Disabled' })).toBeTruthy();
    fireEvent.keyDown(window, { key: 'Escape' });

    fireEvent.click(within(scope).getByRole('button', { name: /^All/ }));
    fireEvent.change(screen.getByPlaceholderText('Title, author, ID, filename'), {
      target: { value: visiblePart.title },
    });
    const bundleSwitch = screen.getByRole('switch', { name: bundle.name });
    expect(bundleSwitch.getAttribute('aria-checked')).toBe('true');
    await act(async () => {
      fireEvent.click(bundleSwitch);
    });
    expect(invokeMock).toHaveBeenCalledWith('toggle_addons', {
      addonIds: [visiblePart.id],
      enabled: false,
      allowWorkshopDir: false,
    });
  });

  it('keeps non-campaign manual and locked dependency bundle members visible', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    const members = snapshot.addons.slice(3, 7);
    snapshot.addons = members;
    snapshot.bundles = [
      {
        id: 'manual',
        name: 'Manual bundle',
        kind: 'manual',
        locked: false,
        memberAddonIds: members.slice(0, 2).map((member) => member.id),
        maps: [],
      },
      {
        id: 'legacy',
        name: 'Legacy bundle',
        kind: 'dependency_graph',
        locked: true,
        memberAddonIds: members.slice(2).map((member) => member.id),
        maps: [],
      },
    ];
    invokeMock.mockResolvedValue(snapshot);
    render(<App />);

    for (const member of members) {
      expect(await screen.findByRole('button', { name: member.title })).toBeTruthy();
    }
    expect(screen.queryByRole('button', { name: 'Group Parts' })).toBeNull();
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

    // Switch Component Name preference to Addon Info, then Filename in Settings -> Appearance
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByRole('button', { name: 'Appearance' }));
    let nameGroup = screen.getByRole('group', { name: 'Component Name' });
    expect(
      within(nameGroup)
        .getByRole('button', { name: 'Workshop' })
        .getAttribute('aria-pressed'),
    ).toBe('true');
    fireEvent.click(within(nameGroup).getByRole('button', { name: 'Addon Info' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    expect(await screen.findByRole('button', { name: 'Yama' })).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByRole('button', { name: 'Appearance' }));
    nameGroup = screen.getByRole('group', { name: 'Component Name' });
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

  it('loads desktop remote covers through the app protocol and falls back to localCoverImage on error', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    const coverUrl = 'http://catalog.example.com/cover.jpg?size=64&v=2';
    snapshot.addons[2].coverImage = coverUrl;
    snapshot.addons[2].localCoverImage = 'abcd1234_cover.jpg';
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'read_cached_image') return [0xff, 0xd8, 0xff];
      return snapshot;
    });
    render(<App />);

    const img = await screen.findByAltText(snapshot.addons[2].title);
    const src = new URL(img.getAttribute('src')!);
    expect(src.protocol).toBe('l4a-cover:');
    expect(src.searchParams.get('url')).toBe(coverUrl);

    // When the remote Workshop cover fails (e.g. offline), AddonCover falls back to the cached local VPK cover
    fireEvent.error(img);
    await waitFor(() => {
      const fallbackImg = screen.getByAltText(snapshot.addons[2].title);
      expect(fallbackImg.getAttribute('src')).toMatch(/^blob:/);
    });
    expect(invokeMock).toHaveBeenCalledWith('read_cached_image', {
      cacheKey: 'abcd1234_cover.jpg',
    });
  });

  it('settles on placeholder without IPC ping-pong when both primary and fallback cache keys are missing', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.addons[2].coverImage = 'missing_primary.jpg';
    snapshot.addons[2].localCoverImage = 'missing_fallback.jpg';
    let cacheReads = 0;
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'read_cached_image') {
        cacheReads += 1;
        throw new Error('not found');
      }
      return snapshot;
    });
    render(<App />);
    await screen.findByText(snapshot.addons[2].title);

    await waitFor(() => {
      expect(cacheReads).toBe(2);
    });
    // Allow microtasks/effects to flush and ensure no infinite re-request loop occurs
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(cacheReads).toBe(2);
    expect(screen.queryByAltText(snapshot.addons[2].title)).toBeNull();
  });

  it('excludes hidden filtered-out addons and non-matching bundle siblings from batch actions', async () => {
    render(<App />);
    await screen.findByText('Yama Campaign Remastered');

    // Selecting standalone addon selects 1 item; selecting 2-part collapsed bundle adds 2 items
    fireEvent.click(screen.getByRole('checkbox', { name: 'Yama Campaign Remastered' }));
    expect(screen.getByText('1 selected')).toBeTruthy();

    fireEvent.click(screen.getByRole('checkbox', { name: 'Chernobyl: Chapter One' }));
    expect(screen.getByText('3 selected')).toBeTruthy();

    // Searching for one bundle member's exact Workshop ID keeps the bundle card visible,
    // but excludes the non-matching sibling (Chernobyl Assets Pack) from batch actions!
    const searchInput = screen.getByPlaceholderText('Title, author, ID, filename');
    fireEvent.change(searchInput, { target: { value: '2814104912' } });
    expect(screen.getByRole('button', { name: 'Chernobyl: Chapter One' })).toBeTruthy();
    expect(screen.getByText('1 selected')).toBeTruthy();
    expect(screen.getByText('2 hidden')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    const deleteDialog = await screen.findByRole('dialog', { name: 'Delete Addons' });
    expect(within(deleteDialog).getByText('Chernobyl: Chapter One')).toBeTruthy();
    expect(within(deleteDialog).queryByText('Chernobyl Assets Pack')).toBeNull();
    fireEvent.keyDown(window, { key: 'Escape' });

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

  it('keeps refreshed Workshop metadata and saved preferences when a refresh overlaps a save', async () => {
    setTauriMode(true);
    const initial = await previewEngine.getDashboardSnapshot();
    const savedConfig = { ...initial.config, collapseCampaignBundles: false };
    const refreshedTitle = 'Hydrated Chernobyl Assets';
    const refreshed: DashboardSnapshot = {
      ...initial,
      addons: initial.addons.map((addon) =>
        addon.title === 'Chernobyl Assets Pack'
          ? { ...addon, title: refreshedTitle, workshopTitle: refreshedTitle }
          : addon,
      ),
    };
    let initialRead = false;
    let finishRefresh!: (snapshot: DashboardSnapshot) => void;
    let finishSave!: (config: DashboardSnapshot['config']) => void;
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'get_dashboard_snapshot') {
        if (!initialRead) {
          initialRead = true;
          return initial;
        }
        return new Promise<DashboardSnapshot>((resolve) => {
          finishRefresh = resolve;
        });
      }
      if (command === 'save_config') {
        return new Promise<DashboardSnapshot['config']>((resolve) => {
          finishSave = resolve;
        });
      }
      return initial;
    });

    await act(async () => {
      render(<App />);
    });
    fireEvent.click(await screen.findByRole('button', { name: 'Group Parts' }));
    await waitFor(() => expect(eventHandler).not.toBeNull());
    act(() => eventHandler?.({ payload: { type: 'snapshot_updated', externalChange: false } }));
    expect(finishRefresh).toBeTypeOf('function');

    await act(async () => finishSave(savedConfig));
    expect(await screen.findByRole('button', { name: 'Chernobyl Assets Pack' })).toBeTruthy();
    await act(async () => finishRefresh(refreshed));

    expect(await screen.findByRole('button', { name: refreshedTitle })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Group Parts' }).getAttribute('aria-pressed')).toBe(
      'false',
    );
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

  it('skips read_cached_image IPC for hide_image and deferred lazy_load exclusions until Load preview is clicked', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.addons[2].localCoverImage = 'yama_lazy_cover.jpg';
    snapshot.addons[3].localCoverImage = 'm4a1_hidden_cover.jpg';
    const nowIso = '2026-11-25T00:00:00Z';
    snapshot.filterExclusions = [
      {
        addonId: snapshot.addons[2].id,
        displayLevel: 'lazy_load',
        bindFingerprint: true,
        boundFingerprint: snapshot.addons[2].contentHash,
        currentFingerprint: snapshot.addons[2].contentHash,
        fingerprintValid: true,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
      {
        addonId: snapshot.addons[3].id,
        displayLevel: 'hide_image',
        bindFingerprint: true,
        boundFingerprint: snapshot.addons[3].contentHash,
        currentFingerprint: snapshot.addons[3].contentHash,
        fingerprintValid: true,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    ];

    const requestedCacheKeys: string[] = [];
    invokeMock.mockImplementation(
      async (
        cmd: string,
        args?: { cacheKey?: string; targets?: FilterEvaluationTarget[] },
      ) => {
        if (cmd === 'read_cached_image') {
          if (args?.cacheKey) requestedCacheKeys.push(args.cacheKey);
          return [0xff, 0xd8, 0xff];
        }
        if (cmd === 'evaluate_filter_items') {
          return {
            policySignature: snapshot.filterPolicySignature ?? '',
            items: (args?.targets ?? []).map((t) => ({
              targetKey: t.targetKey,
              status: 'completed' as const,
              complete: true,
              verdict: {
                targetKey: t.targetKey,
                revisionHint: t.revisionHint,
                flagged: false,
                decisionSource: 'model_safe' as const,
                evaluatedScore: 0.01,
                probabilities: { prob: 0.01, goreProb: 0.005, nsfwProb: 0.005 },
                textSensitive: false,
                titleSensitive: false,
                descriptionSensitive: false,
                imageSensitive: false,
                evaluatedImageIndices: [],
                sensitiveImageIndices: [],
                complete: true,
                evaluatedAt: nowIso,
              },
            })),
          };
        }
        return snapshot;
      },
    );

    render(<App />);
    const yamaBtn = await screen.findByRole('button', { name: 'Yama Campaign Remastered' });
    const m4Btn = screen.getByRole('button', { name: 'Tactical M4A1 SOPMOD' });

    const yamaCard = yamaBtn.closest('article')!;
    const m4Card = m4Btn.closest('article')!;

    // Neither hide_image nor unclicked lazy_load renders <img> or invokes read_cached_image!
    expect(yamaCard.querySelector('img')).toBeNull();
    expect(m4Card.querySelector('img')).toBeNull();
    expect(requestedCacheKeys).toEqual([]);

    // Clicking Load preview on lazy_load loads ONLY that cover without opening the detail sheet!
    const loadPreviewBtn = within(yamaCard).getByRole('button', { name: 'Load preview' });
    fireEvent.click(loadPreviewBtn);

    await waitFor(() => {
      expect(requestedCacheKeys).toEqual(['yama_lazy_cover.jpg']);
      expect(yamaCard.querySelector('img')).not.toBeNull();
    });
    expect(m4Card.querySelector('img')).toBeNull();
    expect(screen.queryByRole('dialog', { name: 'Yama Campaign Remastered' })).toBeNull();
  });

  it('supports blur_card click-once text reveal while keeping image hidden, and subdivides Workshop detail gallery screenshots', async () => {
    const initial = await previewEngine.getDashboardSnapshot();
    await previewEngine.saveConfig({
      ...initial.config,
      filter: {
        ...initial.config.filter,
        enabled: true,
        applyToWorkshop: true,
        flaggedAction: 'blur_card',
        subdivideModalities: true,
        localParams: {
          ...initial.config.filter.localParams,
          maxImages: 4,
        },
      },
    });

    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');

    fireEvent.click(screen.getByRole('button', { name: 'Workshop' }));
    const crimsonTitle = await screen.findByText('Crimson Gore Dissolve FX');
    const card = crimsonTitle.closest('article')!;

    // Zero-flash: card is already blurred with hidden cover and inert inner wrapper even while pending, then settles into reveal button
    expect(card.getAttribute('data-blur-card')).toBe('true');
    expect(card.querySelector('img')).toBeNull();
    expect(card.querySelector('.l4a-blur-card-inner')?.hasAttribute('inert')).toBe(true);

    // First click on settled blurred card overlay: unblurs text & shows description, while STILL keeping image hidden!
    const revealBtn = await within(card).findByRole('button', {
      name: /Sensitive · Click to show text/,
    });
    fireEvent.click(revealBtn);

    expect(card.getAttribute('data-blur-card')).toBe('false');
    expect(
      within(card).getByText(
        'Extreme visceral dismemberment and gore shaders for infected closeups.',
      ),
    ).toBeTruthy();
    expect(card.querySelector('img')).toBeNull();

    // Second click (Inspect) opens Workshop SideSheet: text is readable, safe cover (0) & shot (2) unblurred, gore shot (1) blurred!
    fireEvent.click(within(card).getByRole('button', { name: 'Inspect' }));
    const sheet = await screen.findByRole('dialog', { name: 'Crimson Gore Dissolve FX' });

    expect(
      within(sheet).getByText(
        'Extreme visceral dismemberment and gore shaders for infected closeups.',
      ),
    ).toBeTruthy();

    // Cover image (index 0: #safe_cover) is NOT wrapped in blur container
    const coverImg = within(sheet).getByAltText('Crimson Gore Dissolve FX');
    expect(coverImg.closest('[data-filter-mode="blur"]')).toBeNull();

    // Gallery screenshot #2 (index 1: #gore_viscera_closeup) IS blurred, with working Show image / Blur image peek button
    const goreShotImg = within(sheet).getByAltText('Crimson Gore Dissolve FX #2');
    const goreBlurWrap = goreShotImg.closest('[data-filter-mode="blur"]')!;
    expect(goreBlurWrap).not.toBeNull();
    expect(goreBlurWrap.getAttribute('data-blurred')).toBe('true');

    fireEvent.click(within(sheet).getByRole('button', { name: 'Show image' }));
    expect(goreBlurWrap.getAttribute('data-blurred')).toBe('false');
    fireEvent.click(within(sheet).getByRole('button', { name: 'Blur image' }));
    expect(goreBlurWrap.getAttribute('data-blurred')).toBe('true');

    // Gallery screenshot #3 (index 2: #safe_comparison) remains unblurred
    const safeShotImg = within(sheet).getByAltText('Crimson Gore Dissolve FX #3');
    expect(safeShotImg.closest('[data-filter-mode="blur"]')).toBeNull();
  });

  it('manages fingerprint-bound exclusions across updates, hide_text masking, encrypted LLM token storage, and sandbox testing', async () => {
    const initial = await previewEngine.getDashboardSnapshot();
    await previewEngine.saveConfig({
      ...initial.config,
      collapseCampaignBundles: false,
      filter: {
        ...initial.config.filter,
        enabled: true,
        applyToLibrary: true,
      },
    });

    const { container } = render(<App />);
    await screen.findByText('Chernobyl: Chapter One');
    await waitFor(() => {
      expect(container.querySelector('[data-pending-eval="true"]')).toBeNull();
    });
    fireEvent.click(screen.getByRole('button', { name: 'Chernobyl: Chapter One' }));
    const detailSheet = await screen.findByRole('dialog', { name: 'Chernobyl: Chapter One' });

    // Select Hide Images & Text and add fingerprint-bound exclusion
    const levelSelect = within(detailSheet).getByRole('combobox', {
      name: 'Display Level When Excluded',
    });
    fireEvent.click(levelSelect);
    fireEvent.click(
      within(screen.getByRole('listbox', { name: 'Display Level When Excluded' })).getByRole(
        'option',
        { name: 'Hide Images & Text' },
      ),
    );
    fireEvent.click(within(detailSheet).getByRole('button', { name: 'Add Exclusion' }));

    // Title & raw metadata are now masked by Hide Images & Text on both card and detail sheet!
    expect(
      (await screen.findAllByText('Sensitive title hidden · #2814104912')).length,
    ).toBeGreaterThanOrEqual(2);
    expect(
      within(detailSheet).getByText('Raw metadata masked by Hide Text exclusion level'),
    ).toBeTruthy();
    fireEvent.keyDown(window, { key: 'Escape' });

    // Adopting Workshop update in Reconcile changes contentHash -> invalidates bound exclusion!
    fireEvent.click(screen.getByRole('button', { name: /Reconcile/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Overwrite with Workshop' }));
    const reconcileConfirm = await screen.findByRole('dialog', { name: 'Confirm File Action' });
    fireEvent.click(
      within(reconcileConfirm).getByRole('button', { name: 'Overwrite with Workshop' }),
    );
    await waitFor(() => {
      expect(screen.queryByRole('dialog', { name: 'Confirm File Action' })).toBeNull();
    });

    fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    expect(await screen.findByText('Exclusion hash expired')).toBeTruthy();
    await waitFor(() => {
      expect(container.querySelector('[data-pending-eval="true"]')).toBeNull();
    });

    // Open detail sheet and click Rebind Current Fingerprint -> restores valid exclusion!
    fireEvent.click(screen.getByRole('button', { name: 'Chernobyl: Chapter One' }));
    const invalidatedSheet = await screen.findByRole('dialog', { name: 'Chernobyl: Chapter One' });
    expect(
      within(invalidatedSheet).getByText(
        'Addon fingerprint changed — this exclusion is invalidated',
      ),
    ).toBeTruthy();
    fireEvent.click(
      within(invalidatedSheet).getByRole('button', { name: 'Rebind Current Fingerprint' }),
    );
    expect(
      (await screen.findAllByText('Sensitive title hidden · #2814104912')).length,
    ).toBeGreaterThanOrEqual(2);
    fireEvent.keyDown(window, { key: 'Escape' });

    // Navigate to Settings -> Content Filter: check M3 Select/no-[title] compliance, save encrypted token, and run sandbox
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Content Filter' }));

    await screen.findByText('One-Click Local Deployment (Left4Filters)');
    expect(container.querySelectorAll('select').length).toBe(0);
    expect(container.querySelectorAll('[title]').length).toBe(0);

    const tokenGroup = screen.getByRole('group', { name: 'API Key Source' });
    fireEvent.click(within(tokenGroup).getByRole('button', { name: 'Encrypted Local Store' }));

    const keyInput = screen.getByLabelText('Enter New API Key');
    fireEvent.change(keyInput, { target: { value: 'sk-secret-local-only' } });
    fireEvent.click(screen.getByRole('button', { name: 'Encrypt & Save Token' }));

    expect(await screen.findByText('Encrypted token stored')).toBeTruthy();
    // Plaintext token must NEVER appear in AppConfig!
    const snapAfterToken = await previewEngine.getDashboardSnapshot();
    expect(JSON.stringify(snapAfterToken.config)).not.toContain('sk-secret-local-only');

    // Run Quick Test Sandbox (saving draft first automatically since llmTokenSource changed)
    fireEvent.change(screen.getByLabelText('Sample Title'), {
      target: { value: 'Explicit R18 Bikini Mod' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save & Test' }));

    expect(await screen.findByText('Verdict: Flagged')).toBeTruthy();
  });

  it('enforces Library exclusions with filter disabled, filters collapsed bundle.coverImage, and preserves decimal/negative typing in FilterSettingsSection', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = false;
    snapshot.config.filter.applyToLibrary = false;
    snapshot.config.collapseCampaignBundles = true;
    // Populate explicit bundle.coverImage owned by member addons[0] (which has hide_image exclusion)
    const explicitCoverSvg =
      'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180"><rect width="100%25" height="100%25" fill="%23991111"/></svg>';
    snapshot.addons[0].coverImage = explicitCoverSvg;
    snapshot.bundles[0].coverImage = explicitCoverSvg;
    const nowIso = '2026-11-25T00:00:00Z';
    snapshot.filterExclusions = [
      {
        addonId: 'ws:2814104912',
        displayLevel: 'hide_image',
        bindFingerprint: true,
        boundFingerprint: snapshot.addons[0].contentHash,
        currentFingerprint: snapshot.addons[0].contentHash,
        fingerprintValid: true,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
      {
        addonId: 'ws:3120481120',
        displayLevel: 'hide_text',
        bindFingerprint: true,
        boundFingerprint: snapshot.addons[3].contentHash,
        currentFingerprint: snapshot.addons[3].contentHash,
        fingerprintValid: true,
        createdAt: nowIso,
        updatedAt: nowIso,
      },
    ];

    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'evaluate_filter_items') {
        return { policySignature: snapshot.filterPolicySignature ?? '', items: [] };
      }
      if (cmd === 'get_local_filter_status') {
        return {
          env: {
            gitAvailable: true,
            gitVersion: '2.43.0',
            pythonAvailable: true,
            pythonVersion: '3.12.3',
            venvAvailable: true,
          },
          deployed: false,
          repoUrl: snapshot.config.filter.repoUrl,
          trackLatest: snapshot.config.filter.trackLatest,
          pinnedCommit: snapshot.config.filter.pinnedCommit,
          recentCommits: [],
          updateAvailable: false,
          serviceState: 'stopped',
          serviceEndpoint: null,
          runningPid: null,
          stageMessage: null,
          lastError: null,
        };
      }
      if (cmd === 'get_filter_token_status') {
        return {
          encryptedTokenStored: false,
          envVarName: 'L4A_FILTER_OPENAI_API_KEY',
          envVarPresent: false,
        };
      }
      return snapshot;
    });

    render(<App />);

    // 1. Collapsed bundle with explicit bundle.coverImage still respects member hide_image exclusion (even with filter.enabled === false)!
    const bundleHeading = await screen.findByText('Chernobyl: Chapter One');
    const bundleCard = bundleHeading.closest('article')!;
    expect(bundleCard.querySelector('img')).toBeNull();

    // 2. hide_text exclusion masks Tactical M4A1 SOPMOD even when filter.enabled === false, and prevents raw title search leaks!
    expect(screen.queryByText('Tactical M4A1 SOPMOD')).toBeNull();
    expect(screen.getByText('Sensitive title hidden · #3120481120')).toBeTruthy();

    const searchInput = screen.getByPlaceholderText('Title, author, ID, filename');
    fireEvent.change(searchInput, { target: { value: 'SOPMOD' } });
    expect(screen.queryByText('Sensitive title hidden · #3120481120')).toBeNull();

    fireEvent.change(searchInput, { target: { value: '3120481120' } });
    expect(screen.getByText('Sensitive title hidden · #3120481120')).toBeTruthy();

    // 3. FilterSettingsSection NumericTextField preserves intermediate '-', '-0.', '-0.2', '0.', '0.5', Backspace while focused, and clamps on blur!
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Content Filter' }));

    const biasInput = (await screen.findByLabelText('Logit Bias (-5–5)')) as HTMLInputElement;
    fireEvent.focus(biasInput);
    fireEvent.change(biasInput, { target: { value: '-' } });
    expect(biasInput.value).toBe('-');
    fireEvent.change(biasInput, { target: { value: '-0.' } });
    expect(biasInput.value).toBe('-0.');
    fireEvent.change(biasInput, { target: { value: '-0.2' } });
    expect(biasInput.value).toBe('-0.2');
    fireEvent.blur(biasInput);
    expect(biasInput.value).toBe('-0.2');

    const flagThresholdInput = screen.getByLabelText(
      'Direct Flag Threshold (0–1)',
    ) as HTMLInputElement;
    fireEvent.focus(flagThresholdInput);
    fireEvent.change(flagThresholdInput, { target: { value: '' } });
    expect(flagThresholdInput.value).toBe('');
    fireEvent.change(flagThresholdInput, { target: { value: '0.' } });
    expect(flagThresholdInput.value).toBe('0.');
    fireEvent.change(flagThresholdInput, { target: { value: '0.5' } });
    expect(flagThresholdInput.value).toBe('0.5');
    fireEvent.change(flagThresholdInput, { target: { value: '9.5' } });
    expect(flagThresholdInput.value).toBe('9.5');
    fireEvent.blur(flagThresholdInput);
    expect(flagThresholdInput.value).toBe('1');

    // 4. Threshold overlap warning when llmThreshold >= flagThreshold
    fireEvent.focus(flagThresholdInput);
    fireEvent.change(flagThresholdInput, { target: { value: '0.4' } });
    fireEvent.blur(flagThresholdInput);
    fireEvent.click(
      screen.getByRole('switch', { name: 'Enable External LLM Review for Borderline Scores' }),
    );
    const llmThresholdInput = screen.getByLabelText(
      'LLM Escalation Threshold (0–1)',
    ) as HTMLInputElement;
    fireEvent.focus(llmThresholdInput);
    fireEvent.change(llmThresholdInput, { target: { value: '0.6' } });
    fireEvent.blur(llmThresholdInput);
    expect(
      screen.getByText(/Warning: LLM Escalation Threshold is greater than or equal to Direct Flag Threshold/),
    ).toBeTruthy();

    // 5. Disabling Track latest without a Pinned Commit SHA disables Deploy Now with helper hint
    fireEvent.click(screen.getByRole('switch', { name: 'Track latest remote commit (HEAD)' }));
    expect(
      screen.getByText(
        'Select or enter a target commit SHA before deploying when Track latest is turned off.',
      ),
    ).toBeTruthy();
    expect(
      (screen.getByRole('button', { name: 'Deploy Now' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('shows evaluation progress, logs failed and incomplete checks, and rechecks them from settings', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.collapseCampaignBundles = false;
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.config.filter.flaggedAction = 'blur_card';
    const canonicalSig = computeFilterPolicySignature(snapshot.config.filter, {
      catalogServerUrl: snapshot.config.catalogServerUrl,
      httpsOnly: snapshot.config.httpsOnly,
    });
    expect(canonicalSig).toMatch(/^pol_[0-9a-f]{32}$/);
    snapshot.filterPolicySignature = canonicalSig;

    // Add extra addons so total uncached items > 4 (verifies bounded per-card concurrent evaluation)
    const templateAddon = snapshot.addons[3];
    for (let idx = 0; idx < 5; idx += 1) {
      const wid = `990000${idx}`;
      snapshot.addons.push({
        ...structuredClone(templateAddon),
        id: `ws:${wid}`,
        workshopId: wid,
        title: `Extra Safe Addon ${idx}`,
        addonTitle: `Extra Safe Addon ${idx}`,
        workshopTitle: `Extra Safe Addon ${idx}`,
        contentHash: `hash_extra_${idx}`,
      });
    }

    const nowIso = '2026-11-26T00:00:00Z';
    const validAddon1Rev = buildAddonRevisionHint(snapshot.addons[1]);
    snapshot.cachedFilterVerdicts = [
      {
        // Stale revisionHint for addons[0] (ws:2814104912) -> MUST be ignored and re-evaluated!
        targetKey: 'addon:ws:2814104912',
        revisionHint: 'stale_outdated_revision_000',
        flagged: false,
        decisionSource: 'model_safe',
        evaluatedScore: 0.01,
        probabilities: { prob: 0.01, goreProb: 0.005, nsfwProb: 0.005 },
        textSensitive: false,
        titleSensitive: false,
        descriptionSensitive: false,
        imageSensitive: false,
        evaluatedImageIndices: [],
        sensitiveImageIndices: [],
        complete: true,
        evaluatedAt: nowIso,
      },
      {
        // Matching current revisionHint for addons[1] (ws:2814105100) -> hydrated without re-evaluation!
        targetKey: 'addon:ws:2814105100',
        revisionHint: validAddon1Rev,
        flagged: false,
        decisionSource: 'model_safe',
        evaluatedScore: 0.01,
        probabilities: { prob: 0.01, goreProb: 0.005, nsfwProb: 0.005 },
        textSensitive: false,
        titleSensitive: false,
        descriptionSensitive: false,
        imageSensitive: false,
        evaluatedImageIndices: [],
        sensitiveImageIndices: [],
        complete: true,
        evaluatedAt: nowIso,
      },
    ];

    let inFlightEvalCount = 0;
    let maxObservedConcurrency = 0;
    let evalRound = 0;
    const evaluatedTargetBatches: string[][] = [];
    let releaseSlowCard: (() => void) | null = null;
    const expectedFirstPassCount = snapshot.addons.length - 1;

    invokeMock.mockImplementation(
      async (cmd: string, args?: { targets?: FilterEvaluationTarget[]; config?: AppConfig }) => {
        if (cmd === 'save_config' && args?.config) {
          snapshot.config = structuredClone(args.config);
          return snapshot.config;
        }
        if (cmd === 'get_local_filter_status') {
          return {
            env: { gitAvailable: true, pythonAvailable: true, venvAvailable: true },
            deployed: false,
            serviceState: 'stopped',
            updateAvailable: false,
            recentCommits: [],
          };
        }
        if (cmd === 'get_filter_token_status') {
          return {
            hasEncryptedToken: false,
            envVarName: 'L4A_LLM_API_KEY',
            envVarPresent: false,
          };
        }
        if (cmd === 'evaluate_filter_items') {
          const targets = args?.targets ?? [];
          evaluatedTargetBatches.push(targets.map((t) => t.targetKey));
          inFlightEvalCount += 1;
          maxObservedConcurrency = Math.max(maxObservedConcurrency, inFlightEvalCount);

          if (evalRound === 0 && targets.some((t) => t.targetKey === 'addon:ws:2814104912')) {
            await new Promise<void>((resolve) => {
              releaseSlowCard = resolve;
            });
          }

          inFlightEvalCount -= 1;
          return {
            policySignature: canonicalSig,
            items: targets.map((t) => {
              if (evalRound === 0 && t.targetKey === 'addon:ws:2814104912') {
                return {
                  targetKey: t.targetKey,
                  status: 'completed' as const,
                  complete: false,
                  verdict: {
                    targetKey: t.targetKey,
                    revisionHint: t.revisionHint,
                    flagged: true,
                    decisionSource: 'model_direct_flag' as const,
                    evaluatedScore: 0.93,
                    probabilities: { prob: 0.93, goreProb: 0.01, nsfwProb: 0.92 },
                    textSensitive: false,
                    titleSensitive: false,
                    descriptionSensitive: false,
                    imageSensitive: false,
                    evaluatedImageIndices: [],
                    sensitiveImageIndices: [],
                    complete: false,
                    evaluatedAt: nowIso,
                  },
                };
              }
              if (evalRound === 0 && t.targetKey === 'addon:ws:3011942811') {
                return {
                  targetKey: t.targetKey,
                  status: 'failed' as const,
                  error: 'Preview image fetch failed',
                };
              }
              return {
                targetKey: t.targetKey,
                status: 'completed' as const,
                complete: true,
                verdict: {
                  targetKey: t.targetKey,
                  revisionHint: t.revisionHint,
                  flagged: false,
                  decisionSource: 'model_safe' as const,
                  evaluatedScore: 0.01,
                  probabilities: { prob: 0.01, goreProb: 0.005, nsfwProb: 0.005 },
                  textSensitive: false,
                  titleSensitive: false,
                  descriptionSensitive: false,
                  imageSensitive: false,
                  evaluatedImageIndices: [],
                  sensitiveImageIndices: [],
                  complete: true,
                  evaluatedAt: nowIso,
                },
              };
            }),
          };
        }
        return snapshot;
      },
    );

    render(<App />);

    // Fast cards finish and update immediately while the slow card (Chernobyl: Chapter One) stays pending!
    await waitFor(() => {
      expect(releaseSlowCard).not.toBeNull();
      expect(evaluatedTargetBatches.length).toBe(expectedFirstPassCount);
    });
    expect(evaluatedTargetBatches.every((batch) => batch.length === 1)).toBe(true);
    expect(maxObservedConcurrency).toBeLessThanOrEqual(4);
    const pendingCard = screen.getByText('Chernobyl: Chapter One').closest('article')!;
    expect(
      within(pendingCard).getByRole('progressbar', { name: 'Evaluating content safety…' }),
    ).toBeTruthy();
    expect(within(pendingCard).queryByText('Evaluating content safety…')).toBeNull();
    // Already-finished safe cards are unblurred before the slow card finishes!
    const fastSettledCard = screen.getByText('Tactical M4A1 SOPMOD').closest('article')!;
    expect(fastSettledCard.getAttribute('data-blur-card')).toBe('false');

    // Release slow card -> settles into failure state
    await act(async () => {
      releaseSlowCard?.();
    });
    await waitFor(() => {
      const chernobylCard = screen.getByText('Chernobyl: Chapter One').closest('article')!;
      expect(
        within(chernobylCard).getByRole('button', { name: 'Content check failed · Show text' }),
      ).toBeTruthy();
    });

    const allFirstPassTargets = evaluatedTargetBatches.flat();
    // Stale cached addon:ws:2814104912 WAS re-evaluated; valid cached addon:ws:2814105100 was NOT!
    expect(allFirstPassTargets).toContain('addon:ws:2814104912');
    expect(allFirstPassTargets).not.toContain('addon:ws:2814105100');

    const chernobylCard = screen.getByText('Chernobyl: Chapter One').closest('article')!;
    const yamaCard = screen.getByText('Yama Campaign Remastered').closest('article')!;
    for (const card of [chernobylCard, yamaCard]) {
      expect(
        within(card).getByRole('button', { name: 'Content check failed · Show text' }),
      ).toBeTruthy();
      expect(within(card).queryByRole('button', { name: /Sensitive/ })).toBeNull();
      expect(card.querySelector('img')).toBeNull();
    }
    expect(screen.queryByRole('button', { name: /Retry|Recheck failed items/ })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Content Filter' }));
    const displaySelect = screen.getByRole('combobox', { name: 'Display After a Failed Check' });
    expect(displaySelect.textContent).toBe('Show Failure Notice');
    fireEvent.click(displaySelect);
    fireEvent.click(screen.getByRole('option', { name: 'Use Sensitive Content Display' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(snapshot.config.filter.failureDisplay).toBe('as_flagged'));

    fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    const sensitiveDisplayCard = screen.getByText('Yama Campaign Remastered').closest('article')!;
    expect(
      within(sensitiveDisplayCard).getByRole('button', { name: 'Sensitive · Click to show text' }),
    ).toBeTruthy();
    expect(evaluatedTargetBatches).toHaveLength(expectedFirstPassCount);

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByText('Filter Logs'));
    expect(screen.getByText('Preview image fetch failed')).toBeTruthy();
    expect(screen.getByText('Some preview images could not be loaded for safety evaluation')).toBeTruthy();
    evalRound = 1;
    fireEvent.click(screen.getByRole('button', { name: 'Recheck failed items' }));

    await waitFor(() => {
      expect(evaluatedTargetBatches.length).toBe(expectedFirstPassCount + 2);
      expect(screen.queryByRole('button', { name: 'Recheck failed items' })).toBeNull();
    });
    expect(
      evaluatedTargetBatches.slice(expectedFirstPassCount).flat().sort(),
    ).toEqual([
      'addon:ws:2814104912',
      'addon:ws:3011942811',
    ]);
    expect(screen.getByText('Preview image fetch failed')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    await waitFor(() => {
      for (const title of ['Chernobyl: Chapter One', 'Yama Campaign Remastered']) {
        expect(screen.getByText(title).closest('article')?.getAttribute('data-blur-card')).toBe('false');
      }
    });

    // Native CoreEvent filter_verdicts_updated with canonical pol_... policySignature is accepted!
    await act(async () => {
      eventHandler?.({
        payload: {
          type: 'filter_verdicts_updated',
          policySignature: canonicalSig,
          verdicts: [
            {
              targetKey: 'addon:ws:3120481120',
              revisionHint: buildAddonRevisionHint(snapshot.addons[3]),
              flagged: true,
              decisionSource: 'model_direct_flag',
              evaluatedScore: 0.93,
              probabilities: { prob: 0.93, goreProb: 0.92, nsfwProb: 0.01 },
              textSensitive: true,
              titleSensitive: true,
              descriptionSensitive: false,
              imageSensitive: true,
              evaluatedImageIndices: [],
              sensitiveImageIndices: [],
              complete: true,
              evaluatedAt: '2026-11-26T00:01:00Z',
            },
          ],
        },
      });
    });

    const m4Card = screen.getByText('Tactical M4A1 SOPMOD').closest('article')!;
    expect(m4Card.getAttribute('data-blur-card')).toBe('true');
  });

  it('preserves newly saved enabled state and thresholds when deploying local filter from dirty settings', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = false;
    snapshot.config.filter.flagThreshold = 0.72;
    const savedConfigs: AppConfig[] = [];

    invokeMock.mockImplementation(
      async (cmd: string, args?: { config?: AppConfig; targets?: FilterEvaluationTarget[] }) => {
        if (cmd === 'save_config' && args?.config) {
          const cloned = structuredClone(args.config);
          savedConfigs.push(cloned);
          snapshot.config = cloned;
          return cloned;
        }
        if (cmd === 'deploy_local_filter') {
          return {
            env: {
              gitAvailable: true,
              gitVersion: '2.45.2',
              pythonAvailable: true,
              pythonVersion: '3.12.3',
              venvAvailable: true,
            },
            deployed: true,
            repoDir: '/tmp/left4filters/repo',
            venvDir: '/tmp/left4filters/venv',
            serviceState: 'running',
            serviceEndpoint: 'http://127.0.0.1:18791',
            runningPid: 12345,
            updateAvailable: false,
            recentCommits: [],
            stageMessage: 'Deployed latest commit',
          };
        }
        if (cmd === 'get_local_filter_status') {
          return {
            env: {
              gitAvailable: true,
              gitVersion: '2.45.2',
              pythonAvailable: true,
              pythonVersion: '3.12.3',
              venvAvailable: true,
            },
            deployed: false,
            repoDir: '/tmp/left4filters/repo',
            venvDir: '/tmp/left4filters/venv',
            serviceState: 'stopped',
            updateAvailable: false,
            recentCommits: [],
          };
        }
        if (cmd === 'get_filter_token_status') {
          return {
            hasEncryptedToken: false,
            envVarName: 'L4A_LLM_API_KEY',
            envVarPresent: false,
          };
        }
        if (cmd === 'evaluate_filter_items') {
          return {
            policySignature: computeFilterPolicySignature(snapshot.config.filter, {
              catalogServerUrl: snapshot.config.catalogServerUrl,
              httpsOnly: snapshot.config.httpsOnly,
            }),
            items: (args?.targets ?? []).map((t) => ({
              targetKey: t.targetKey,
              status: 'completed' as const,
              complete: true,
              verdict: {
                targetKey: t.targetKey,
                revisionHint: t.revisionHint,
                flagged: false,
                decisionSource: 'model_safe' as const,
                evaluatedScore: 0.01,
                probabilities: { prob: 0.01, goreProb: 0.005, nsfwProb: 0.005 },
                textSensitive: false,
                titleSensitive: false,
                descriptionSensitive: false,
                imageSensitive: false,
                evaluatedImageIndices: [],
                sensitiveImageIndices: [],
                complete: true,
                evaluatedAt: '2026-11-26T00:00:00Z',
              },
            })),
          };
        }
        return snapshot;
      },
    );

    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Content Filter' }));

    const enableSwitch = await screen.findByRole('switch', {
      name: 'Enable Sensitive Content Filter',
    });
    expect(enableSwitch.getAttribute('aria-checked')).toBe('false');

    // Dirty edits: enable filter + set Direct Flag Threshold to 0.55, then immediately click Deploy Now!
    fireEvent.click(enableSwitch);
    const flagInput = screen.getByLabelText('Direct Flag Threshold (0–1)') as HTMLInputElement;
    fireEvent.focus(flagInput);
    fireEvent.change(flagInput, { target: { value: '0.55' } });
    fireEvent.blur(flagInput);

    const deployBtn = await screen.findByRole('button', { name: 'Deploy Now' });
    fireEvent.click(deployBtn);

    await screen.findByText('Deployed latest commit');
    expect(savedConfigs.length).toBeGreaterThanOrEqual(1);
    expect(savedConfigs[0].filter.enabled).toBe(true);
    expect(savedConfigs[0].filter.flagThreshold).toBe(0.55);

    // After deploy completes, newly saved enabled=true and flagThreshold=0.55 MUST remain intact!
    expect(
      screen
        .getByRole('switch', { name: 'Enable Sensitive Content Filter' })
        .getAttribute('aria-checked'),
    ).toBe('true');
    expect((screen.getByLabelText('Direct Flag Threshold (0–1)') as HTMLInputElement).value).toBe(
      '0.55',
    );
  });

  it('omits hidden-member bundle.coverImage on UI cards and includes hiddenCard addon changes in ProfilesTab and OrganizeInspectorSheet with redacted names', async () => {
    const initial = await previewEngine.getDashboardSnapshot();
    // Make addons[1] (ws:2814105100, in Chernobyl bundle) an explicit NSFW item that will become hiddenCard under hide_card!
    const hiddenMemberCover =
      'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180"><rect width="100%25" height="100%25" fill="%23ff00aa"/></svg>';
    initial.addons[0].coverImage = undefined;
    initial.addons[0].localCoverImage = undefined;
    initial.addons[1].title = 'Explicit R18 Bikini Assets Pack';
    initial.addons[1].addonTitle = 'Explicit R18 Bikini Assets Pack';
    initial.addons[1].workshopTitle = 'Explicit R18 Bikini Assets Pack';
    initial.addons[1].enabled = false;
    initial.addons[1].coverImage = hiddenMemberCover;
    // Bundle cover points to the hidden member's image!
    initial.bundles[0].coverImage = hiddenMemberCover;

    // Profile enables ONLY already-enabled addons PLUS the disabled hidden addon (ws:2814105100), so ONLY the hidden addon is in toEnable!
    const currentlyEnabledLoadingIds = initial.addons
      .filter((a) => a.installState === 'installed' && a.primaryDir === 'loading' && a.enabled)
      .map((a) => a.id);
    await previewEngine.saveProfile({
      id: 'prof-hidden-diff',
      name: 'Hidden Diff Loadout',
      description: 'Toggles a hidden addon on disk',
      enabledAddonIds: ['ws:2814105100', ...currentlyEnabledLoadingIds],
    });

    // Mutate previewEngine state via saveConfig + updating addons/bundles through previewEngine reset/snapshot
    // Wait: previewEngine.getDashboardSnapshot() clones state, so let's run in Tauri mock mode with full snapshot control!
    setTauriMode(true);
    initial.profiles = [
      {
        id: 'prof-hidden-diff',
        name: 'Hidden Diff Loadout',
        description: 'Toggles a hidden addon on disk',
        enabledAddonIds: ['ws:2814105100', ...currentlyEnabledLoadingIds],
      },
    ];
    initial.config.collapseCampaignBundles = true;
    initial.config.filter.enabled = true;
    initial.config.filter.applyToLibrary = true;
    initial.config.filter.flaggedAction = 'hide_card';
    const sig = computeFilterPolicySignature(initial.config.filter, {
      catalogServerUrl: initial.config.catalogServerUrl,
      httpsOnly: initial.config.httpsOnly,
    });
    initial.filterPolicySignature = sig;

    invokeMock.mockImplementation(
      async (cmd: string, args?: { targets?: FilterEvaluationTarget[] }) => {
        if (cmd === 'evaluate_filter_items') {
          return {
            policySignature: sig,
            items: (args?.targets ?? []).map((t) => {
              const isFlagged = t.targetKey === 'addon:ws:2814105100';
              return {
                targetKey: t.targetKey,
                status: 'completed' as const,
                complete: true,
                verdict: {
                  targetKey: t.targetKey,
                  revisionHint: t.revisionHint,
                  flagged: isFlagged,
                  decisionSource: isFlagged
                    ? ('model_direct_flag' as const)
                    : ('model_safe' as const),
                  evaluatedScore: isFlagged ? 0.94 : 0.01,
                  probabilities: {
                    prob: isFlagged ? 0.94 : 0.01,
                    goreProb: 0.01,
                    nsfwProb: isFlagged ? 0.94 : 0.005,
                  },
                  textSensitive: isFlagged,
                  titleSensitive: isFlagged,
                  descriptionSensitive: false,
                  imageSensitive: isFlagged,
                  evaluatedImageIndices: isFlagged ? [0] : [],
                  sensitiveImageIndices: isFlagged ? [0] : [],
                  complete: true,
                  evaluatedAt: '2026-11-26T00:00:00Z',
                },
              };
            }),
          };
        }
        return initial;
      },
    );

    const { container } = render(<App />);
    const bundleTitle = await screen.findByText('Chernobyl: Chapter One');
    await waitFor(() => {
      expect(container.querySelector('[data-pending-eval="true"]')).toBeNull();
    });

    // 1. Collapsed bundle in Library MUST NOT display the hidden member's cover image!
    const libBundleCard = bundleTitle.closest('article')!;
    expect(libBundleCard.querySelector('img')).toBeNull();

    // 2. Organize -> Bundles tab also omits the hidden member's cover image!
    fireEvent.click(screen.getByRole('button', { name: 'Organize' }));
    const orgBundleMatches = await screen.findAllByText('Chernobyl: Chapter One');
    const orgBundleCard = orgBundleMatches[0].closest('article')!;
    expect(orgBundleCard.querySelector('img')).toBeNull();

    // 3. Organize -> Profiles tab includes the hidden addon in Enable +1 (NOT 'No changes'!) and redacts its chip name!
    fireEvent.click(screen.getByRole('tab', { name: /Profiles/ }));
    const profileTitle = await screen.findByText('Hidden Diff Loadout');
    const profileCard = profileTitle.closest('article')!;
    expect(within(profileCard).queryByText('No changes')).toBeNull();
    expect(within(profileCard).getByText('Enable +1')).toBeTruthy();
    expect(
      within(profileCard).getByText('Sensitive title hidden · #2814105100'),
    ).toBeTruthy();
    expect(within(profileCard).queryByText('Explicit R18 Bikini Assets Pack')).toBeNull();

    // 4. Clicking the profile opens OrganizeInspectorSheet showing Enable +1 and the redacted row!
    fireEvent.click(within(profileCard).getByRole('button', { name: /Hidden Diff Loadout/ }));
    const inspector = await screen.findByRole('dialog', { name: 'Hidden Diff Loadout' });
    expect(within(inspector).getAllByText('Enable +1').length).toBeGreaterThanOrEqual(1);
    expect(
      within(inspector).getByText('Sensitive title hidden · #2814105100'),
    ).toBeTruthy();
    expect(within(inspector).queryByText('Explicit R18 Bikini Assets Pack')).toBeNull();
  });

  it('preserves invalidated exclusion bindings on display-level change, locks presets in Catalog mode, and redacts download/update task titles and toasts', async () => {
    setTauriMode(true);
    const initial = await previewEngine.getDashboardSnapshot();
    const targetAddon = {
      ...initial.addons[0],
      id: 'ws:2814199999',
      workshopId: '2814199999',
      title: 'Top Secret Sensitive Addon',
      workshopTitle: 'Top Secret Sensitive Addon',
      addonTitle: 'Top Secret Sensitive Addon',
      installState: 'installed' as const,
      updateAvailable: true,
      contentHash: 'hash-v2-changed',
    };
    initial.addons = [targetAddon];
    initial.bundles = [];
    initial.filterExclusions = [
      {
        addonId: targetAddon.id,
        displayLevel: 'hide_text',
        bindFingerprint: true,
        boundFingerprint: 'hash-v1-original',
        fingerprintValid: false,
        createdAt: '2026-11-26T00:00:00Z',
        updatedAt: '2026-11-26T00:00:00Z',
      },
    ];
    initial.config = {
      ...initial.config,
      filter: {
        ...initial.config.filter,
        enabled: true,
        mode: 'catalog',
        applyToLibrary: true,
        flaggedAction: 'blur_card',
      },
    };
    // Mark addon flagged with titleSensitive so even with invalidated exclusion it stays title-redacted
    initial.cachedFilterVerdicts = [
      {
        targetKey: `addon:${targetAddon.id}`,
        revisionHint: undefined,
        flagged: true,
        decisionSource: 'model_direct_flag',
        evaluatedScore: 0.95,
        probabilities: { prob: 0.95, goreProb: 0.01, nsfwProb: 0.95 },
        textSensitive: true,
        titleSensitive: true,
        descriptionSensitive: false,
        imageSensitive: true,
        evaluatedImageIndices: [0],
        sensitiveImageIndices: [0],
        complete: true,
        evaluatedAt: '2026-11-26T00:00:00Z',
      },
    ];

    const upsertCalls: Array<Record<string, unknown>> = [];
    const updateCalls: Array<Record<string, unknown>> = [];

    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'get_dashboard_snapshot' || cmd === 'scan_workspace') {
        return initial;
      }
      if (cmd === 'get_local_filter_status') {
        return {
          deployed: false,
          serviceState: 'not_deployed',
          env: { gitAvailable: true, pythonAvailable: true, venvAvailable: true },
          updateAvailable: false,
          recentCommits: [],
        };
      }
      if (cmd === 'get_filter_token_status') {
        return {
          hasEncryptedLocalToken: false,
          envVarName: 'L4A_LLM_API_KEY',
          envVarPresent: false,
          dpapiProtected: false,
        };
      }
      if (cmd === 'evaluate_filter_items') {
        const targets = (args?.targets as Array<{ targetKey: string; revisionHint?: string }>) ?? [];
        return {
          policySignature: initial.filterPolicySignature ?? 'pol_test',
          cacheGeneration: 0,
          items: targets.map((t) => ({
            targetKey: t.targetKey,
            status: 'completed',
            complete: true,
            verdict: {
              ...initial.cachedFilterVerdicts![0],
              targetKey: t.targetKey,
              revisionHint: t.revisionHint,
            },
          })),
        };
      }
      if (cmd === 'upsert_filter_exclusion') {
        const req = (args?.request as Record<string, unknown>) ?? {};
        upsertCalls.push(req);
        return initial;
      }
      if (cmd === 'update_workshop_item') {
        updateCalls.push(args ?? {});
        return targetAddon;
      }
      return initial;
    });

    render(<App />);

    // 1. Go to Settings -> Content Filter
    fireEvent.click(await screen.findByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Content Filter' }));

    // Preset selector must be disabled in Catalog mode
    const presetSelect = await screen.findByRole('combobox', { name: 'Parameter Preset' });
    expect((presetSelect as HTMLButtonElement).disabled).toBe(true);

    // Changing invalidated exclusion's display level MUST omit bindFingerprint (no silent rebind!)
    const levelSelect = screen.getByRole('combobox', { name: 'Display Level When Excluded' });
    fireEvent.click(levelSelect);
    fireEvent.click(
      within(screen.getByRole('listbox', { name: 'Display Level When Excluded' })).getByRole(
        'option',
        { name: /Blur/ },
      ),
    );
    await waitFor(() => expect(upsertCalls.length).toBe(1));
    expect(upsertCalls[0]).toEqual({
      addonId: targetAddon.id,
      displayLevel: 'blur_image',
      note: undefined,
    });
    expect('bindFingerprint' in upsertCalls[0]).toBe(false);

    // 2. Open the excluded addon detail sheet and click Update:
    //    both update_workshop_item titleHint and the completion toast must NOT leak the raw title!
    fireEvent.click(screen.getByRole('button', { name: /#2814199999/ }));
    const sheet = await screen.findByRole('dialog');
    fireEvent.click(within(sheet).getByRole('button', { name: 'Update' }));

    await waitFor(() => expect(updateCalls.length).toBe(1));
    expect(updateCalls[0].titleHint).toBeUndefined();
    await waitFor(() => {
      expect(screen.getByText('Updated · Sensitive title hidden · #2814199999')).toBeTruthy();
    });
    expect(screen.queryByText(/Updated · Top Secret Sensitive Addon/)).toBeNull();
  });

  it('saves only filter config on Deploy Now without flushing unsaved path edits and shields pending Direct Inspect text in WorkshopView', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToWorkshop = true;
    snapshot.config.filter.flaggedAction = 'blur_card';
    snapshot.config.filter.mode = 'local';

    const saveCalls: Array<{ config: AppConfig; forceRescan?: boolean }> = [];
    let scanCalls = 0;

    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'get_dashboard_snapshot') {
        return snapshot;
      }
      if (cmd === 'scan_workspace') {
        scanCalls += 1;
        return snapshot;
      }
      if (cmd === 'save_config') {
        const nextCfg = args?.config as AppConfig;
        saveCalls.push({ config: structuredClone(nextCfg) });
        snapshot.config = structuredClone(nextCfg);
        return snapshot.config;
      }
      if (cmd === 'deploy_local_filter') {
        return {
          deployed: true,
          serviceState: 'running',
          env: { gitAvailable: true, pythonAvailable: true, venvAvailable: true },
          updateAvailable: false,
          recentCommits: [],
          cacheGeneration: 5,
        };
      }
      if (cmd === 'get_local_filter_status') {
        return {
          deployed: false,
          serviceState: 'not_deployed',
          env: { gitAvailable: true, pythonAvailable: true, venvAvailable: true },
          updateAvailable: false,
          recentCommits: [],
          cacheGeneration: 0,
        };
      }
      if (cmd === 'get_filter_token_status') {
        return {
          hasEncryptedLocalToken: false,
          envVarName: 'L4A_LLM_API_KEY',
          envVarPresent: false,
          dpapiProtected: false,
          cacheGeneration: 0,
        };
      }
      if (cmd === 'search_workshop') {
        return { items: [], total: 0, page: 1, pageSize: 24, source: 'catalog_server' };
      }
      if (cmd === 'fetch_workshop_item') {
        return {
          workshopId: '99887766',
          fileType: 'item',
          status: 'active',
          title: 'Unverified Explicit Mod Title',
          description: 'Unverified Explicit Description Body',
          creatorName: 'SecretAuthorName',
          tags: ['Survivors'],
          dependencies: [],
          collectionChildren: [],
          source: 'catalog_server',
          fetchedAt: '2026-11-26T00:00:00Z',
        };
      }
      if (cmd === 'evaluate_filter_items') {
        const targets = (args?.targets as Array<{ targetKey: string; revisionHint?: string }>) ?? [];
        if (targets.some((t) => t.targetKey === 'ws:99887766')) {
          // Keep Direct Inspect evaluation pending so SideSheet renders while unverified!
          return new Promise(() => {});
        }
        return {
          policySignature: snapshot.filterPolicySignature ?? 'pol_test',
          cacheGeneration: 0,
          items: targets.map((t) => ({
            targetKey: t.targetKey,
            status: 'completed',
            complete: true,
            verdict: {
              targetKey: t.targetKey,
              revisionHint: t.revisionHint,
              flagged: false,
              decisionSource: 'model_safe',
              evaluatedScore: 0.01,
              probabilities: { prob: 0.01, goreProb: 0.005, nsfwProb: 0.005 },
              textSensitive: false,
              titleSensitive: false,
              descriptionSensitive: false,
              imageSensitive: false,
              evaluatedImageIndices: [],
              sensitiveImageIndices: [],
              complete: true,
              evaluatedAt: '2026-11-26T00:00:00Z',
            },
          })),
        };
      }
      return snapshot;
    });

    render(<App />);
    await screen.findByText('Chernobyl: Chapter One');

    // 1. Edit Loading Dir on Paths tab (unsaved), then edit a Filter setting and click Deploy Now
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    const loadingInput = (await screen.findByLabelText(
      'Manual Addons Directory (addons)',
    )) as HTMLInputElement;
    const originalLoadingDir = loadingInput.value;
    fireEvent.change(loadingInput, { target: { value: '/tmp/unsaved-custom-loading-dir' } });

    fireEvent.click(screen.getByRole('button', { name: 'Content Filter' }));
    const flagInput = (await screen.findByLabelText('Direct Flag Threshold (0–1)')) as HTMLInputElement;
    fireEvent.focus(flagInput);
    fireEvent.change(flagInput, { target: { value: '0.61' } });
    fireEvent.blur(flagInput);

    const scanCallsBeforeDeploy = scanCalls;
    fireEvent.click(await screen.findByRole('button', { name: 'Deploy Now' }));
    await waitFor(() => expect(saveCalls.length).toBe(1));
    expect(saveCalls[0].config.filter.flagThreshold).toBe(0.61);
    expect(saveCalls[0].config.loadingDir).toBe(originalLoadingDir);
    expect(scanCalls).toBe(scanCallsBeforeDeploy);

    // Switching back to Paths tab still preserves the unsaved loadingDir edit
    fireEvent.click(screen.getByRole('button', { name: 'Paths' }));
    expect(
      ((await screen.findByLabelText('Manual Addons Directory (addons)')) as HTMLInputElement)
        .value,
    ).toBe('/tmp/unsaved-custom-loading-dir');

    // 2. Navigate to Workshop and Direct Inspect an unverified item under blur_card
    fireEvent.click(screen.getByRole('button', { name: 'Workshop' }));
    const directInput = await screen.findByPlaceholderText('Workshop ID or URL');
    fireEvent.change(directInput, { target: { value: '99887766' } });
    fireEvent.click(screen.getByRole('button', { name: 'Inspect' }));

    const sheet = await screen.findByRole('dialog');
    expect(within(sheet).getByText(/Sensitive title hidden · #99887766/)).toBeTruthy();
    expect(within(sheet).queryByText('Unverified Explicit Mod Title')).toBeNull();
    expect(within(sheet).queryByText('SecretAuthorName')).toBeNull();
    expect(within(sheet).queryByText('Unverified Explicit Description Body')).toBeNull();
    expect(within(sheet).queryByRole('button', { name: /show text/i })).toBeNull();
  });

  it.each([
    ['notice', 'Content check failed · Show text'],
    ['as_flagged', 'Sensitive · Click to show text'],
  ] as const)('reveals directly inspected Workshop text after a partial check (%s)', async (
    failureDisplay,
    revealLabel,
  ) => {
    const { config } = await previewEngine.getDashboardSnapshot();
    await previewEngine.saveConfig({
      ...config,
      filter: {
        ...config.filter,
        enabled: true,
        applyToLibrary: false,
        flaggedAction: 'blur_card',
        failureDisplay,
      },
    });
    const workshopId = '66778899';
    const title = 'Direct Inspect Preview';
    const description = 'Description for the directly inspected item';
    const fetchItem = vi.spyOn(previewEngine, 'fetchWorkshopItem').mockResolvedValue({
      workshopId,
      fileType: 'item',
      status: 'active',
      title,
      description,
      previewUrl: 'https://example.test/gore-cover.jpg',
      previews: [{ previewType: 0, url: 'error://gallery-unavailable' }],
      tags: [],
      dependencies: [],
      collectionChildren: [],
      source: 'catalog_server',
      fetchedAt: '2026-11-26T00:00:00Z',
    });

    try {
      render(<App />);
      await screen.findByText('Chernobyl: Chapter One');
      fireEvent.click(screen.getByRole('button', { name: 'Workshop' }));
      const directInput = await screen.findByPlaceholderText('Workshop ID or URL');
      fireEvent.change(directInput, { target: { value: workshopId } });
      fireEvent.keyDown(directInput, { key: 'Enter' });

      const sheet = within(await screen.findByRole('dialog'));
      const reveal = await sheet.findByRole('button', { name: revealLabel });
      expect(sheet.queryByText(title)).toBeNull();
      expect(sheet.queryByText(description)).toBeNull();
      expect(sheet.queryByText('Flagged Sensitive Content')).toBeNull();

      fireEvent.click(reveal);
      expect(sheet.getByRole('heading', { name: title })).toBeTruthy();
      expect(sheet.getByText(description)).toBeTruthy();
      expect(sheet.getByAltText(title).closest('[data-blurred="true"]')).toBeTruthy();
    } finally {
      fetchItem.mockRestore();
    }
  });

  it('shields flagged Direct Inspect text in WorkshopView until explicitly revealed and redacts AddonPicker author under blur_card', async () => {
    setTauriMode(true);
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.collapseCampaignBundles = false;
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.config.filter.applyToWorkshop = true;
    snapshot.config.filter.flaggedAction = 'blur_card';
    snapshot.addons[3].author = 'HiddenPickerAuthor';
    const sig = computeFilterPolicySignature(snapshot.config.filter, {
      catalogServerUrl: snapshot.config.catalogServerUrl,
      httpsOnly: snapshot.config.httpsOnly,
    });
    snapshot.filterPolicySignature = sig;

    invokeMock.mockImplementation(
      async (cmd: string, args?: { targets?: FilterEvaluationTarget[] }) => {
        if (cmd === 'search_workshop') {
          return { items: [], total: 0, page: 1, pageSize: 24, source: 'catalog_server' };
        }
        if (cmd === 'fetch_workshop_item') {
          return {
            workshopId: '66778899',
            fileType: 'item',
            status: 'active',
            title: 'Explicit Flagged Direct Mod',
            description: 'Explicit Flagged Direct Description',
            creatorName: 'ExplicitCreator',
            tags: ['ExplicitTag'],
            dependencies: [],
            collectionChildren: [],
            source: 'catalog_server',
            fetchedAt: '2026-11-26T00:00:00Z',
          };
        }
        if (cmd === 'evaluate_filter_items') {
          const targets = args?.targets ?? [];
          return {
            policySignature: sig,
            cacheGeneration: 0,
            items: targets.map((t) => {
              const isFlagged =
                t.targetKey === 'ws:66778899' || t.targetKey === `addon:${snapshot.addons[3].id}`;
              return {
                targetKey: t.targetKey,
                status: 'completed' as const,
                complete: true,
                verdict: {
                  targetKey: t.targetKey,
                  revisionHint: t.revisionHint,
                  flagged: isFlagged,
                  decisionSource: isFlagged
                    ? ('model_direct_flag' as const)
                    : ('model_safe' as const),
                  evaluatedScore: isFlagged ? 0.96 : 0.01,
                  probabilities: { prob: isFlagged ? 0.96 : 0.01, goreProb: 0.01, nsfwProb: isFlagged ? 0.96 : 0.01 },
                  textSensitive: isFlagged,
                  titleSensitive: isFlagged,
                  descriptionSensitive: isFlagged,
                  imageSensitive: isFlagged,
                  evaluatedImageIndices: [],
                  sensitiveImageIndices: [],
                  complete: true,
                  evaluatedAt: '2026-11-26T00:00:00Z',
                },
              };
            }),
          };
        }
        return snapshot;
      },
    );

    const { container } = render(<App />);
    await screen.findByText('Chernobyl: Chapter One');
    await waitFor(() => {
      expect(container.querySelector('[data-pending-eval="true"]')).toBeNull();
    });

    // 1. Organize -> Groups -> New Group picker must redact author of unrevealed blur_card flagged addon
    fireEvent.click(screen.getByRole('button', { name: 'Organize' }));
    fireEvent.click(await screen.findByRole('tab', { name: /Groups/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'New Group' }));
    const groupDialog = await screen.findByRole('dialog', { name: 'New Group' });
    expect(within(groupDialog).queryByText('HiddenPickerAuthor')).toBeNull();
    fireEvent.keyDown(window, { key: 'Escape' });

    // 2. Workshop Direct Inspect of a flagged mod shields sensitive text until user clicks reveal button
    fireEvent.click(screen.getByRole('button', { name: 'Workshop' }));
    const directInput = await screen.findByPlaceholderText('Workshop ID or URL');
    fireEvent.change(directInput, { target: { value: '66778899' } });
    fireEvent.click(screen.getByRole('button', { name: 'Inspect' }));

    const sheet = await screen.findByRole('dialog');
    const revealBtn = await within(sheet).findByRole('button', {
      name: /Sensitive · Click to show text/,
    });
    expect(within(sheet).queryByText('Explicit Flagged Direct Mod')).toBeNull();
    expect(within(sheet).queryByText('ExplicitCreator')).toBeNull();
    expect(within(sheet).queryByText('Explicit Flagged Direct Description')).toBeNull();

    // Clicking the reveal button exposes the text cleanly
    fireEvent.click(revealBtn);
    expect(await within(sheet).findByText('Explicit Flagged Direct Mod')).toBeTruthy();
    expect(within(sheet).getByText('ExplicitCreator')).toBeTruthy();
    expect(within(sheet).getByText('Explicit Flagged Direct Description')).toBeTruthy();
  });
});
