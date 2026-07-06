import { describe, test, expect } from 'vitest';
import {
  formatBytes,
  getAddonCategories,
  getImageUrl,
  getAddonUrl,
  getAddonAuthor,
  getSuggestedVpkName,
} from './addonHelpers';
import { Addon } from '../types/addon';

describe('addonHelpers', () => {
  describe('formatBytes', () => {
    test('should format 0 bytes correctly', () => {
      expect(formatBytes(0)).toBe('0 Bytes');
    });

    test('should format bytes to KB, MB, GB correctly', () => {
      expect(formatBytes(1024)).toBe('1 KB');
      expect(formatBytes(1536)).toBe('1.5 KB');
      expect(formatBytes(1024 * 1024)).toBe('1 MB');
      expect(formatBytes(1024 * 1024 * 1024 * 1.5)).toBe('1.5 GB');
    });
  });

  describe('getAddonCategories', () => {
    test('should return Campaign when addonContent_Campaign is 1', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {
          addonContent_Campaign: '1',
        },
      };
      expect(getAddonCategories(addon)).toContain('Campaign');
    });

    test('should match category keys case-insensitively', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {
          addoncontent_campaign: '1',
          ADDONCONTENT_SURVIVOR: '1',
        } as any,
      };
      const cats = getAddonCategories(addon);
      expect(cats).toContain('Campaign');
      expect(cats).toContain('Survivor');
    });

    test('should ignore keys with value 0', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {
          addonContent_Campaign: '0',
          addonContent_Survivor: 0,
          addonContent_Map: '1',
        },
      };
      const cats = getAddonCategories(addon);
      expect(cats).toContain('Map');
      expect(cats).not.toContain('Campaign');
      expect(cats).not.toContain('Survivor');
    });

    test('should fall back to Other when no categories are present', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {},
      };
      expect(getAddonCategories(addon)).toEqual(['Other']);
    });

    test('should extract categories from steam details tags', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        steamDetails: {
          tags: [{ tag: 'Campaign' }, { tag: 'weapon' }],
        },
      };
      const cats = getAddonCategories(addon);
      expect(cats).toContain('Campaign');
      expect(cats).toContain('Weapon Model');
    });
  });

  describe('getImageUrl', () => {
    test('should return empty string for undefined path', () => {
      expect(getImageUrl()).toBe('');
    });

    test('should return same URL if it starts with http or https', () => {
      expect(getImageUrl('https://example.com/test.jpg')).toBe('https://example.com/test.jpg');
      expect(getImageUrl('http://example.com/test.jpg')).toBe('http://example.com/test.jpg');
    });

    test('should convert cache path to localhost cache url', () => {
      const cachePath = '/cache/abc_image.jpg';
      const result = getImageUrl(cachePath);
      expect(result).toSatisfy((val: string) => val.includes('localhost/abc_image.jpg'));
    });
  });

  describe('getAddonUrl', () => {
    test('should return correct URL when available', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {
          addonurl0: 'https://github.com',
        },
      };
      expect(getAddonUrl(addon)).toBe('https://github.com');
    });

    test('should prepend https to url if it has domain structure', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {
          addonurl: 'github.com',
        },
      };
      expect(getAddonUrl(addon)).toBe('https://github.com');
    });

    test('should return null when URL is invalid or empty', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {
          addonurl: 'abc',
        },
      };
      expect(getAddonUrl(addon)).toBeNull();
    });
  });

  describe('getAddonAuthor', () => {
    test('should return addonAuthor if present in addonInfo', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        addonInfo: {
          addonAuthor: 'Author A',
        },
      };
      expect(getAddonAuthor(addon)).toBe('Author A');
    });

    test('should return steamDetails creator_name if addonInfo has no author', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        steamDetails: {
          creator_name: 'Creator Name',
        },
      };
      expect(getAddonAuthor(addon)).toBe('Creator Name');
    });

    test('should return Unknown Author if no author info is present', () => {
      const addon: Addon = {
        vpkName: 'test.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
      };
      expect(getAddonAuthor(addon)).toBe('Unknown Author');
    });
  });

  describe('getSuggestedVpkName', () => {
    test('should strip brackets and suggest clean vpk name for non-workshop item', () => {
      const addon: Addon = {
        vpkName: '[OldGroup]my_addon.vpk',
        dirType: 'loading',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
      };
      const suggested = getSuggestedVpkName(addon, 'NewGroup', {});
      expect(suggested).toBe('[NewGroup]my_addon.vpk');
    });

    test('should suggest name with workshopId prefix for workshop items', () => {
      const addon: Addon = {
        vpkName: '2938529557.vpk',
        dirType: 'workshop',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        workshopId: '2938529557',
        steamDetails: {
          title: 'Cool Campaign Map',
        },
      };
      const suggested = getSuggestedVpkName(addon, 'Campaigns', {});
      expect(suggested).toBe('[2938529557][Campaigns]Cool Campaign Map.vpk');
    });

    test('should avoid name conflicts by appending a counter', () => {
      const addon: Addon = {
        vpkName: 'temp.vpk',
        dirType: 'workshop',
        isEnabled: true,
        fileSize: 100,
        filesCount: 1,
        workshopId: '123',
        steamDetails: {
          title: 'Cool Map'
        }
      };
      const existing: Record<string, Addon> = {
        '[123]Cool Map.vpk': {
          vpkName: '[123]Cool Map.vpk',
          dirType: 'loading',
          isEnabled: true,
          fileSize: 100,
          filesCount: 1,
        }
      };
      const suggested = getSuggestedVpkName(addon, undefined, existing);
      expect(suggested).toBe('[123]Cool Map_1.vpk');
    });
  });
});
