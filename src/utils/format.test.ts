import { describe, expect, it } from 'vitest';
import type { AddonSummary } from '../api/ipc';
import {
  extractWorkshopIdInput,
  formatAddonDisplayName,
  formatBundleMemberLabel,
  formatBytes,
  formatSpeed,
  getFileBaseName,
  previewFilenameByRules,
  resolveAddonMetadataTitle,
  stripLeadingBracketPrefixes,
  stripVpkExtension,
  summarizeBatchReport,
} from './format';

function makeSampleAddon(overrides: Partial<AddonSummary>): AddonSummary {
  return {
    id: 'hash:sample',
    title: 'Sample Addon',
    installState: 'installed',
    primaryDir: 'loading',
    enabled: true,
    fileSize: 1024,
    filesCount: 1,
    categories: ['Campaign'],
    maps: [],
    hasDummyBypass: false,
    remoteStatus: 'unknown',
    updateAvailable: false,
    addonInfo: {},
    ...overrides,
  };
}

describe('format utilities', () => {
  it('formats byte sizes cleanly', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1024 * 1024 * 12.5)).toBe('12.5 MB');
  });

  it('formats transfer speeds including explicit 0 B/s', () => {
    expect(formatSpeed(0)).toBe('0 B/s');
    expect(formatSpeed(-100)).toBe('0 B/s');
    expect(formatSpeed(Number.NaN)).toBe('0 B/s');
    expect(formatSpeed(1536)).toBe('1.5 KB/s');
    expect(formatSpeed(1024 * 1024 * 12.5)).toBe('12.5 MB/s');
  });

  it('validates positive u64 workshop IDs from numbers, Steam HTTPS URLs, and steam:// URIs', () => {
    expect(extractWorkshopIdInput('2814104912')).toBe('2814104912');
    expect(extractWorkshopIdInput('00042')).toBe('42');
    expect(
      extractWorkshopIdInput('https://steamcommunity.com/sharedfiles/filedetails/?id=3499001245'),
    ).toBe('3499001245');
    expect(extractWorkshopIdInput('steam://url/CommunityFilePage/2814104912')).toBe('2814104912');
    expect(extractWorkshopIdInput('0')).toBeNull();
    expect(extractWorkshopIdInput('000')).toBeNull();
    expect(extractWorkshopIdInput('18446744073709551616')).toBeNull();
    expect(extractWorkshopIdInput('not-a-valid-id')).toBeNull();
  });

  it('extracts file base names across Windows and POSIX paths', () => {
    expect(getFileBaseName('C:\\Games\\L4D2\\addons\\sample.vpk')).toBe('sample.vpk');
    expect(getFileBaseName('/steam/addons/workshop/12345.vpk')).toBe('12345.vpk');
  });

  it('switches cleanly across workshop_title, addon_title, and file_name preferences', () => {
    expect(stripVpkExtension('[12345] yama_part1.vpk.disabled')).toBe('[12345] yama_part1');
    const addon = makeSampleAddon({
      title: 'Yama Campaign Remastered',
      workshopTitle: 'Yama Campaign Remastered',
      addonTitle: 'Yama',
      currentFileName: '[3011942811] Yama_Campaign.vpk.disabled',
    });
    expect(formatAddonDisplayName(addon)).toBe('Yama Campaign Remastered');
    expect(formatAddonDisplayName(addon, 'workshop_title')).toBe('Yama Campaign Remastered');
    expect(formatAddonDisplayName(addon, 'addon_title')).toBe('Yama');
    expect(formatAddonDisplayName(addon, 'file_name')).toBe('[3011942811] Yama_Campaign');
    expect(resolveAddonMetadataTitle(addon, 'workshop_title')).toBe('Yama Campaign Remastered');
    expect(resolveAddonMetadataTitle(addon, 'addon_title')).toBe('Yama');
  });

  it('disambiguates multi-part bundle members with identical or generic addon titles', () => {
    const localPart1 = makeSampleAddon({
      id: 'hash:p1',
      title: 'Silent Hill: Otherside',
      addonTitle: 'Silent Hill: Otherside',
      currentFileName: 'silenthill_part1.vpk',
    });
    const localPart2 = makeSampleAddon({
      id: 'hash:p2',
      title: 'Silent Hill: Otherside',
      addonTitle: 'Silent Hill: Otherside',
      currentFileName: 'silenthill_part2.vpk',
    });
    const siblings = [localPart1, localPart2];
    expect(
      formatBundleMemberLabel(localPart1, siblings, 'addon_title', 'Silent Hill: Otherside'),
    ).toBe('Silent Hill: Otherside · silenthill_part1');
    expect(
      formatBundleMemberLabel(localPart2, siblings, 'addon_title', 'Silent Hill: Otherside'),
    ).toBe('Silent Hill: Otherside · silenthill_part2');
    expect(
      formatBundleMemberLabel(localPart1, siblings, 'file_name', 'Silent Hill: Otherside'),
    ).toBe('silenthill_part1');

    // Also disambiguates when Part 1 has generic addonTitle == bundleName while Part 2 fell back to filename
    const mixedPart1 = makeSampleAddon({
      id: 'hash:m1',
      title: 'Yama',
      addonTitle: 'Yama',
      currentFileName: 'yama_1.vpk',
    });
    const mixedPart2 = makeSampleAddon({
      id: 'hash:m2',
      title: 'yama_2',
      currentFileName: 'yama_2.vpk',
    });
    expect(
      formatBundleMemberLabel(mixedPart1, [mixedPart1, mixedPart2], 'addon_title', 'Yama'),
    ).toBe('Yama · yama_1');

    // Under file_name preference, colliding filenames never leak workshopTitle
    const collidingFileA = makeSampleAddon({
      id: 'ws:101',
      workshopId: '101',
      title: 'Yama (Part 1)',
      workshopTitle: 'Yama (Part 1)',
      currentFileName: 'shared_name.vpk',
    });
    const collidingFileB = makeSampleAddon({
      id: 'ws:102',
      workshopId: '102',
      title: 'Yama (Part 2)',
      workshopTitle: 'Yama (Part 2)',
      currentFileName: 'shared_name.vpk',
    });
    expect(
      formatBundleMemberLabel(
        collidingFileA,
        [collidingFileA, collidingFileB],
        'file_name',
        'Yama',
      ),
    ).toBe('shared_name · #101');
  });

  it('falls back to file stem when metadata titles are SDK placeholders and strips bracket prefixes', () => {
    const placeholderAddon = makeSampleAddon({
      title: 'My First Add-on',
      addonTitle: 'My First Add-on',
      addonInfo: { addontitle: 'My Add-on' },
      currentFileName: 'sakura_campaign_第1部分.vpk',
    });
    expect(resolveAddonMetadataTitle(placeholderAddon)).toBe('sakura_campaign_第1部分');
    expect(stripLeadingBracketPrefixes('[2814104912] [Campaigns] Chernobyl')).toBe('Chernobyl');
  });

  it('uses filenames when both metadata titles collide across bundle members', () => {
    const parts = ['north', 'south'].map((name) =>
      makeSampleAddon({
        id: `hash:${name}`,
        title: 'City Escape Campaign',
        workshopTitle: 'City Escape Campaign',
        addonTitle: 'City Escape',
        currentFileName: `${name}.vpk`,
      }),
    );
    expect(parts.map((part) => formatBundleMemberLabel(part, parts))).toEqual([
      'City Escape Campaign · north',
      'City Escape Campaign · south',
    ]);
    expect(parts.map((part) => formatBundleMemberLabel(part, parts, 'addon_title'))).toEqual([
      'City Escape · north',
      'City Escape · south',
    ]);

    parts[1].addonTitle = 'City Escape Part 2';
    expect(formatBundleMemberLabel(parts[1], parts)).toBe('City Escape Part 2');
  });

  it.each([
    ['underscore', '[2814104912] [Campaigns] Chernobyl_Chapter One (v2)_.vpk'],
    ['space', '[2814104912] [Campaigns] Chernobyl Chapter One (v2).vpk'],
    ['empty', '[2814104912] [Campaigns] ChernobylChapter One (v2).vpk'],
  ] as const)('previews permitted punctuation with %s replacement', (replacement, expected) => {
    const out = previewFilenameByRules(
      {
        enableWorkshopIdPrefix: true,
        enableGroupPrefix: true,
        cleanSpecialChars: true,
        invalidCharReplace: replacement,
        maxFilenameLength: 0,
        enableTrim: true,
        enableRemoveDoubleSpaces: true,
      },
      {
        workshopId: '2814104912',
        groupName: 'Campaigns',
        title: '  Chernobyl:Chapter  One (v2)! ',
      },
    );
    expect(out).toBe(expected);
  });

  it('summarizes batch reports accurately', () => {
    const summary = summarizeBatchReport({
      items: [
        { addonId: 'a', status: 'applied' },
        { addonId: 'b', status: 'unchanged' },
        { addonId: 'c', status: 'failed', error: { code: 'conflict', message: 'exists' } },
        { addonId: 'd', status: 'not_attempted' },
      ],
    });
    expect(summary).toEqual({
      applied: 1,
      unchanged: 1,
      failed: 1,
      notAttempted: 1,
      hasIssue: true,
    });
  });
});
