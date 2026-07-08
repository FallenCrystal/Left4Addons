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

  test('fetchWorkshopItems enriches Steam SDK items with HTML author data when SDK lacks persona names', async () => {
    mockParseSSRItems.mockReturnValue([
      {
        workshopId: '12345',
        title: 'SDK Item',
        imagePath: '/cache/item.png',
        authorName: 'Visible Author',
        authorId: '76561198000000001',
        authorUrl: 'https://steamcommunity.com/profiles/76561198000000001',
        authorSteamId: '76561198000000001',
        stars: 5,
      },
    ]);
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          canQueryItems: true,
          canQueryHome: true,
        });
      }
      if (cmd === 'query_workshop_items') {
        return Promise.resolve({
          source: 'steam-sdk',
          items: [{
            publishedfileid: '12345',
            title: 'SDK Item',
            creator: '76561198000000001',
            creator_steam_id: '76561198000000001',
            creator_account_id: '39734273',
            creator_name: '',
            score: 0.96,
          }],
        });
      }
      if (cmd === 'fetch_workshop_html') {
        return Promise.resolve('<html></html>');
      }
      throw new Error(`Unexpected command: ${cmd}`);
    });

    const workshopClient = await import('./workshopClient');
    const result = await workshopClient.fetchWorkshopItems({ query: 'tank' });

    expect(result.source).toBe('steam-sdk');
    expect(result.items).toHaveLength(1);
    expect(result.items[0]).toMatchObject({
      authorName: 'Visible Author',
      authorId: '76561198000000001',
      authorUrl: 'https://steamcommunity.com/profiles/76561198000000001',
      stars: 5,
    });
    expect(mockInvoke).toHaveBeenCalledWith('fetch_workshop_html', expect.objectContaining({
      source: 'workshop-search',
    }));
  });

  test('mapSteamDetailToWorkshopItem keeps 64-bit creator id and converts score to stars', async () => {
    const workshopClient = await import('./workshopClient');
    const item = workshopClient.mapSteamDetailToWorkshopItem({
      publishedfileid: '12345',
      title: 'SDK Item',
      creator: '76561198000000001',
      creator_steam_id: '76561198000000001',
      creator_account_id: '39734273',
      creator_name: '',
      score: 0.81,
    });

    expect(item.authorId).toBe('76561198000000001');
    expect(item.authorSteamId).toBe('76561198000000001');
    expect(item.authorAccountId).toBe('39734273');
    expect(item.authorName).toBe('');
    expect(item.stars).toBe(4);
  });

  test('fetchWorkshopItems defaults text queries to textsearch sort when none is provided', async () => {
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          canQueryItems: true,
          canQueryHome: true,
        });
      }
      if (cmd === 'query_workshop_items') {
        expect(args).toEqual(expect.objectContaining({
          query: expect.objectContaining({
            query: 'Early Days',
            sort: 'textsearch',
          }),
        }));
        return Promise.resolve({ source: 'steam-sdk', items: [] });
      }
      throw new Error(`Unexpected command: ${cmd}`);
    });

    const workshopClient = await import('./workshopClient');
    const result = await workshopClient.fetchWorkshopItems({ query: 'Early Days' });

    expect(result.source).toBe('steam-sdk');
    expect(result.items).toEqual([]);
  });

  test('fetchWorkshopItems reports Steam SDK warnings to the configured reporter', async () => {
    const reporter = vi.fn();
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          canQueryItems: true,
          canQueryHome: true,
        });
      }
      if (cmd === 'query_workshop_items') {
        return Promise.resolve({
          source: 'steam-sdk',
          items: [],
          warnings: ['Steamworks SDK creator persona lookup failed for 1 author(s): 76561198000000001'],
        });
      }
      throw new Error(`Unexpected command: ${cmd}`);
    });

    const workshopClient = await import('./workshopClient');
    workshopClient.setWorkshopWarningReporter(reporter);

    await workshopClient.fetchWorkshopItems({ query: 'test' });

    expect(reporter).toHaveBeenCalledWith(
      'Steamworks SDK creator persona lookup failed for 1 author(s): 76561198000000001',
    );

    workshopClient.setWorkshopWarningReporter(null);
  });
});
