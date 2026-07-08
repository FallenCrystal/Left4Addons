import { beforeEach, describe, expect, test, vi } from 'vitest';

const {
  mockInvoke,
  mockParseHomepageSections,
  mockParseSSRItems,
  mockParseTagCategories,
  mockRememberWorkshopItems,
} = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockParseHomepageSections: vi.fn(),
  mockParseSSRItems: vi.fn(),
  mockParseTagCategories: vi.fn(),
  mockRememberWorkshopItems: vi.fn((items: unknown) => items),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mockInvoke(cmd, args),
}));

vi.mock('../components/workshop/ssrParser', () => ({
  parseHomepageSections: (html: string) => mockParseHomepageSections(html),
  parseSSRItems: (html: string, source: string) => mockParseSSRItems(html, source),
  parseTagCategories: (html: string) => mockParseTagCategories(html),
  parseWorkshopPageDetails: vi.fn(),
}));

vi.mock('../components/workshop/authorDirectory', () => ({
  rememberWorkshopItems: (items: unknown) => mockRememberWorkshopItems(items),
  rememberWorkshopPageDetails: vi.fn(),
}));

describe('workshopClient', () => {
  beforeEach(() => {
    vi.resetModules();
    mockInvoke.mockReset();
    mockParseHomepageSections.mockReset();
    mockParseSSRItems.mockReset();
    mockParseTagCategories.mockReset();
    mockRememberWorkshopItems.mockClear();
  });

  test('fetchWorkshopItems skips Steamworks SDK queries when disabled in frontend settings', async () => {
    mockParseSSRItems.mockReturnValue([
      {
        workshopId: '12345',
        title: 'Fallback Item',
        imagePath: '/cache/item.png',
        authorName: 'Author',
        authorId: '42',
        authorUrl: 'https://steamcommunity.com/id/author',
        stars: 0,
      },
    ]);
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'fetch_workshop_html') {
        return Promise.resolve('<html></html>');
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          canQueryItems: true,
          canQueryHome: true,
        });
      }
      if (cmd === 'query_workshop_items') {
        return Promise.resolve({ source: 'steam-sdk', items: [] });
      }
      throw new Error(`Unexpected command: ${cmd}`);
    });

    const workshopClient = await import('./workshopClient');
    workshopClient.setSteamworksSdkDisabled(true);

    const result = await workshopClient.fetchWorkshopItems({ query: 'tank' });

    expect(result.source).toBe('web-fallback');
    expect(result.items).toHaveLength(1);
    expect(mockInvoke).not.toHaveBeenCalledWith('get_workshop_capabilities', undefined);
    expect(mockInvoke).not.toHaveBeenCalledWith('query_workshop_items', expect.anything());
    expect(mockInvoke).toHaveBeenCalledWith('fetch_workshop_html', expect.objectContaining({
      source: 'workshop-search',
    }));
  });

  test('fetchWorkshopHome skips Steamworks SDK queries when disabled in frontend settings', async () => {
    mockParseHomepageSections.mockReturnValue([
      {
        id: 'recent',
        title: 'recent',
        subtitle: 'recent desc',
        icon: null,
        items: [],
        browseParams: {
          sort: 'trend',
          section: 'readytouseitems',
        },
      },
    ]);
    mockParseTagCategories.mockReturnValue([]);
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'fetch_workshop_html') {
        return Promise.resolve('<html></html>');
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          canQueryItems: true,
          canQueryHome: true,
        });
      }
      if (cmd === 'query_workshop_home') {
        return Promise.resolve({ source: 'steam-sdk', sections: [] });
      }
      throw new Error(`Unexpected command: ${cmd}`);
    });

    const workshopClient = await import('./workshopClient');
    workshopClient.setSteamworksSdkDisabled(true);

    const result = await workshopClient.fetchWorkshopHome();

    expect(result.source).toBe('web-fallback');
    expect(result.sections).toHaveLength(1);
    expect(mockInvoke).not.toHaveBeenCalledWith('get_workshop_capabilities', undefined);
    expect(mockInvoke).not.toHaveBeenCalledWith('query_workshop_home', undefined);
    expect(mockInvoke).toHaveBeenCalledWith('fetch_workshop_html', expect.objectContaining({
      source: 'workshop-home',
    }));
  });
});
