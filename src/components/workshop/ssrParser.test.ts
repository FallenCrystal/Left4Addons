import { describe, expect, test } from 'vitest';
import { parseWorkshopPageDetails, parseSSRItems } from './ssrParser';

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

  test('uses the current Steam preview image as the cover when no gallery exists', () => {
    const details = parseWorkshopPageDetails(`
      <div class="workshopItemTitle">Lingshan-Guangxi V2.9</div>
      <div id="highlight_player_area">
        <img
          id="previewImage"
          class="workshopItemPreviewImageEnlargeable"
          src="https://images.steamusercontent.com/ugc/10303821298862575212/4771C25DD397BD581B244C8A37355A1AB7603C04/?imw=637&amp;imh=358&amp;ima=fit"
        >
      </div>
      <script>var rgFullScreenshotURLs = [];</script>
    `);

    expect(details.previewUrl).toBe(
      'https://images.steamusercontent.com/ugc/10303821298862575212/4771C25DD397BD581B244C8A37355A1AB7603C04/',
    );
    expect(details.imageGallery).toEqual([]);
  });

  test('correctly parses Chinese detail stats, author prefix, and tag categories', () => {
    const html = `
      <div class="workshopItemTitle">测试武器 MOD</div>
      <div class="friendBlock" data-miniprofile="123456">
        <a class="friendBlockLinkOverlay" href="https://steamcommunity.com/id/testuser"></a>
        <div class="friendBlockContent">创作者：testuser<br><span>在线</span></div>
      </div>
      <div class="detailsStatsContainerLeft">
        <div class="detailsStatLeft">文件大小</div>
        <div class="detailsStatLeft">发表于</div>
        <div class="detailsStatLeft">更新日期</div>
      </div>
      <div class="detailsStatsContainerRight">
        <div class="detailsStatRight">12.34 MB</div>
        <div class="detailsStatRight">8 月 1 日 上午 10:00</div>
        <div class="detailsStatRight">8 月 10 日 下午 2:30</div>
      </div>
      <table class="stats_table">
        <tr><td>1,234</td><td>不重复访客数</td></tr>
        <tr><td>567</td><td>当前订阅者</td></tr>
        <tr><td>89</td><td>当前收藏人数</td></tr>
      </table>
      <div class="rightDetailsBlock">
        <div class="workshopTags">
          <div class="workshopTagsTitle">游戏内容：</div>
          <a href="#">武器</a>
        </div>
      </div>
    `;

    const details = parseWorkshopPageDetails(html);
    expect(details.creatorName).toBe('testuser');
    expect(details.fileSizeDisplay).toBe('12.34 MB');
    expect(details.postedDateText).toBe('8 月 1 日 上午 10:00');
    expect(details.updatedDateText).toBe('8 月 10 日 下午 2:30');
    expect(details.uniqueVisitors).toBe(1234);
    expect(details.currentSubscribers).toBe(567);
    expect(details.currentFavorites).toBe(89);
    expect(details.tags).toEqual([{ category: '游戏内容', name: '武器' }]);
  });

  test('correctly strips Chinese author prefixes from DOM cards in parseSSRItems', () => {
    const html = `
      <div class=" Panel">
        <div>
          <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=9990001">
            <img src="https://images.steamusercontent.com/test.jpg">
          </a>
        </div>
        <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=9990001">Mod Title</a>
        <a href="https://steamcommunity.com/profiles/76561198877666030/myworkshopfiles">创作者：TaNtaL1c</a>
      </div>
    `;

    const items = parseSSRItems(html, 'workshop_browse');
    expect(items.length).toBe(1);
    expect(items[0].authorName).toBe('TaNtaL1c');
  });
});
