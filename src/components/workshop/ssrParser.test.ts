import { describe, expect, test } from 'vitest';
import { parseWorkshopPageDetails } from './ssrParser';

describe('ssrParser', () => {
  test('preserves Steam description line breaks from rendered HTML', () => {
    const details = parseWorkshopPageDetails(`
      <div class="workshopItemTitle">Early Days PART 1/6</div>
      <div class="friendBlock" data-miniprofile="51754853">
        <a class="friendBlockLinkOverlay" href="https://steamcommunity.com/id/perfectbuddy"></a>
        <div class="friendBlockContent">perfect_buddy<br><span>Offline</span></div>
      </div>
      <div class="workshopItemDescription" id="highlightContent">
        <b>UPDATE:</b> Fixed things.<br><br>
        <div class="bb_h2">Gameplay</div>
        First line.<br>Second line.
        <ul class="bb_ul"><li>Residential<br></li><li>Downtown</li></ul>
      </div>
    `);

    expect(details.description).toContain('UPDATE: Fixed things.');
    expect(details.description).toContain('\n\nGameplay\n');
    expect(details.description).toContain('First line.\nSecond line.');
    expect(details.description).toContain('* Residential');
    expect(details.creatorName).toBe('perfect_buddy');
    expect(details.creatorSteamId).toBe('76561198012020581');
  });
});
