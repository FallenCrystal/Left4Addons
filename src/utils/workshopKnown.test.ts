import { describe, expect, test } from 'vitest';
import { findKnownWorkshopEntry, isKnownWorkshopItem } from './workshopKnown';

describe('workshop known entries', () => {
  test('uses the raw workshop ID as the identifier', () => {
    const entry = { id: '12345', workshopId: '12345' };
    const entries = { '12345': entry };

    expect(findKnownWorkshopEntry(entries, '12345')).toBe(entry);
    expect(isKnownWorkshopItem(entries, '12345')).toBe(true);
  });

  test('does not treat a VPK filename as a workshop identifier', () => {
    const entries = {
      '12345.vpk': { id: '12345.vpk', workshopId: undefined },
    };

    expect(findKnownWorkshopEntry(entries, '12345')).toBeUndefined();
    expect(isKnownWorkshopItem(entries, '12345')).toBe(false);
  });
});
