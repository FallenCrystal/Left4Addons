import { afterEach, describe, expect, test } from 'vitest';
import type { WorkshopItem, WorkshopPageDetails } from './types';
import {
  __resetWorkshopAuthorDirectoryForTests,
  rememberWorkshopItems,
  rememberWorkshopPageDetails,
  resolveWorkshopItemAuthor,
} from './authorDirectory';

function createItem(overrides: Partial<WorkshopItem>): WorkshopItem {
  return {
    workshopId: '100',
    title: 'Test Item',
    imagePath: '',
    authorName: '',
    authorId: '',
    authorUrl: '',
    stars: 0,
    ...overrides,
  };
}

afterEach(() => {
  __resetWorkshopAuthorDirectoryForTests();
});

describe('authorDirectory', () => {
  test('reuses author name across pages when another page only has steam id', () => {
    const namedItem = createItem({
      workshopId: '101',
      authorName: 'Real Author',
      authorId: '76561198000000001',
      authorSteamId: '76561198000000001',
      authorUrl: 'https://steamcommunity.com/profiles/76561198000000001',
    });

    rememberWorkshopItems([namedItem]);

    const idOnlyItem = createItem({
      workshopId: '102',
      authorName: '76561198000000001',
      authorId: '76561198000000001',
      authorSteamId: '76561198000000001',
    });

    const resolved = resolveWorkshopItemAuthor(idOnlyItem);
    expect(resolved.authorName).toBe('Real Author');
    expect(resolved.authorUrl).toBe('https://steamcommunity.com/profiles/76561198000000001');
  });

  test('does not let numeric placeholder overwrite a known author name', () => {
    rememberWorkshopItems([
      createItem({
        workshopId: '101',
        authorName: 'Real Author',
        authorId: '76561198000000001',
        authorSteamId: '76561198000000001',
      }),
    ]);

    rememberWorkshopItems([
      createItem({
        workshopId: '102',
        authorName: '76561198000000001',
        authorId: '76561198000000001',
        authorSteamId: '76561198000000001',
      }),
    ]);

    const resolved = resolveWorkshopItemAuthor(createItem({
      workshopId: '103',
      authorName: '76561198000000001',
      authorId: '76561198000000001',
      authorSteamId: '76561198000000001',
    }));

    expect(resolved.authorName).toBe('Real Author');
  });

  test('learns author identity from workshop page details', () => {
    const details: WorkshopPageDetails = {
      creatorName: 'Page Author',
      creatorProfileUrl: 'https://steamcommunity.com/profiles/76561198000000002',
      creatorSteamId: '76561198000000002',
      imageGallery: [],
      tags: [],
      requiredItems: [],
      parentCollections: [],
    };

    rememberWorkshopPageDetails(details);

    const resolved = resolveWorkshopItemAuthor(createItem({
      workshopId: '104',
      authorName: '76561198000000002',
      authorId: '76561198000000002',
      authorSteamId: '76561198000000002',
    }));

    expect(resolved.authorName).toBe('Page Author');
    expect(resolved.authorUrl).toBe('https://steamcommunity.com/profiles/76561198000000002');
  });

  test('matches Steam account id to SteamID64 for page author mappings', () => {
    rememberWorkshopPageDetails({
      creatorName: 'perfect_buddy',
      creatorProfileUrl: 'https://steamcommunity.com/id/perfectbuddy',
      creatorAccountId: '51754853',
      imageGallery: [],
      tags: [],
      requiredItems: [],
      parentCollections: [],
    });

    const resolved = resolveWorkshopItemAuthor(createItem({
      workshopId: '105',
      authorName: '76561198012020581',
      authorId: '76561198012020581',
      authorSteamId: '76561198012020581',
    }));

    expect(resolved.authorName).toBe('perfect_buddy');
    expect(resolved.authorUrl).toBe('https://steamcommunity.com/id/perfectbuddy');
  });
});
