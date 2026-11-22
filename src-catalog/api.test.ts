import { describe, expect, it } from 'vitest';
import { formatBytes, parseWorkshopIdsInput } from './api';

describe('catalog web console helpers', () => {
  it('extracts deduplicated Workshop IDs from mixed text and Steam URLs', () => {
    const raw = `
      https://steamcommunity.com/sharedfiles/filedetails/?id=2814104912
      2814105100, 2814104912; id=3499001245
    `;
    expect(parseWorkshopIdsInput(raw)).toEqual([
      '2814104912',
      '2814105100',
      '3499001245',
    ]);
  });

  it('formats byte sizes cleanly', () => {
    expect(formatBytes(undefined)).toBe('—');
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1024)).toBe('1.0 KB');
    expect(formatBytes(486_539_264)).toBe('464.0 MB');
  });
});
