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

  test('parses collection children from Steam collection page DOM', () => {
    const details = parseWorkshopPageDetails(`
      <div class="workshopItemTitle">Early Days Campaign</div>
      <div class="workshopItemDescription">
        <b>Subscribe to all 6 parts.</b><br><br>Please rate part 1.
      </div>
      <div class="collectionChildren">
        <div id="sharedfile_3560883926" class="collectionItem">
          <div class="workshopItem">
            <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=3560883926">
              <div class="workshopItemPreviewHolder">
                <img class="workshopItemPreviewImage" src="https://images.steamusercontent.com/ugc/preview1/?imw=200&amp;imh=200">
              </div>
            </a>
          </div>
          <div class="collectionItemDetails">
            <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=3560883926">
              <div class="workshopItemTitle">Early Days PART 1/6</div>
            </a>
            <div class="workshopItemAuthor">
              Created by
              <span class="workshopItemAuthorName">
                <a href="https://steamcommunity.com/id/perfectbuddy/myworkshopfiles?appid=550">perfect_buddy</a>
              </span>
            </div>
            <img class="fileRating" src="https://community.fastly.steamstatic.com/public/images/sharedfiles/5-star.png">
            <div class="workshopItemShortDesc">UPDATE: Some performance improvements...</div>
          </div>
        </div>
        <div id="sharedfile_3560886114" class="collectionItem">
          <div class="collectionItemDetails">
            <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=3560886114">
              <div class="workshopItemTitle">Early Days PART 2/6</div>
            </a>
            <div class="workshopItemAuthor">
              Created by
              <span class="workshopItemAuthorName">
                <a href="https://steamcommunity.com/id/perfectbuddy/myworkshopfiles?appid=550">perfect_buddy</a>
              </span>
            </div>
          </div>
        </div>
      </div>
    `);

    expect(details.description).toContain('Subscribe to all 6 parts.');
    expect(details.childItemIds).toEqual(['3560883926', '3560886114']);
    expect(details.collectionItems?.map((item) => item.title)).toEqual([
      'Early Days PART 1/6',
      'Early Days PART 2/6',
    ]);
    expect(details.collectionItems?.[0].authorName).toBe('perfect_buddy');
    expect(details.collectionItems?.[0].authorVanityId).toBe('perfectbuddy');
    expect(details.collectionItems?.[0].stars).toBe(5);
  });
});
