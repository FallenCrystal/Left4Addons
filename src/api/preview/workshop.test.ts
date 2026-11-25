import { describe, expect, it } from 'vitest';
import { searchPreviewWorkshop } from './workshop';

describe('preview Workshop search', () => {
  it.each([
    'steam://url/CommunityFilePage/3499001245/',
    'STEAM://url/CommunityFilePage/0003499001245/',
    'https://steamcommunity.com/sharedfiles/filedetails/?ID=0003499001245',
  ])('finds a collection by its Workshop link %s', (query) => {
    const response = searchPreviewWorkshop({ query });

    expect(response.total).toBe(1);
    expect(response.items[0]?.workshopId).toBe('3499001245');
    expect(response.items[0]?.fileType).toBe('collection');
  });
});
