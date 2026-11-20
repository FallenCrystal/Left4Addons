import { describe, expect, it } from 'vitest';
import {
  extractWorkshopIdInput,
  formatBytes,
  getFileBaseName,
  previewFilenameByRules,
  summarizeBatchReport,
} from './format';

describe('format utilities', () => {
  it('formats byte sizes cleanly', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1024 * 1024 * 12.5)).toBe('12.5 MB');
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
