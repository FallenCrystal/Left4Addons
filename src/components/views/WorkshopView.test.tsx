import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import i18n, { setAppLanguage } from '../../i18n';
import { IpcError, type SearchQuery, type SearchResponse, type WorkshopItemRecord } from '../../api/ipc';
import { WorkshopView } from './WorkshopView';

function sampleRecord(id: number): WorkshopItemRecord {
  return {
    workshopId: String(id),
    fileType: 'item',
    status: 'active',
    title: `Campaign ${id}`,
    description: 'Sample campaign description',
    tags: ['Campaigns'],
    previews: [],
    dependencies: [],
    collectionChildren: [],
    source: 'catalog_server',
    fetchedAt: '2026-11-24T00:00:00Z',
  };
}

function renderWorkshop(onSearch: (query: SearchQuery) => Promise<SearchResponse>) {
  return render(
    <WorkshopView
      addons={[]}
      collections={[]}
      liveDownloads={{}}
      onSearch={onSearch}
      onFetchItem={async () => null}
      onImportSubscribed={async () => null}
      onDownload={async () => null}
      onUpdate={async () => null}
      onCancelDownload={() => {}}
      onTrackCollection={async () => null}
      onOpenSourcesSettings={() => {}}
      onOpenExternal={() => {}}
      onInvalidId={() => {}}
      onNotify={() => {}}
    />,
  );
}

describe('WorkshopView', () => {
  beforeEach(async () => {
    setAppLanguage('en');
    await i18n.changeLanguage('en');
  });

  afterEach(async () => {
    setAppLanguage('en');
    await i18n.changeLanguage('en');
    cleanup();
  });

  it('resets pagination to page 1 when UI language changes', async () => {
    const calls: SearchQuery[] = [];
    const onSearch = vi.fn(async (query: SearchQuery): Promise<SearchResponse> => {
      calls.push(query);
      return {
        items: [sampleRecord(query.page ?? 1)],
        total: 48,
        page: query.page ?? 1,
        pageSize: 24,
        source: 'catalog_server',
      };
    });

    renderWorkshop(onSearch);

    await waitFor(() => {
      expect(calls.length).toBeGreaterThanOrEqual(1);
    });
    expect(calls[0]).toMatchObject({ page: 1, language: 'en' });

    const nextBtn = await screen.findByRole('button', { name: 'Next' });
    fireEvent.click(nextBtn);

    await waitFor(() => {
      expect(calls.some((c) => c.page === 2 && c.language === 'en')).toBe(true);
    });

    const countBeforeLangSwitch = calls.length;
    await act(async () => {
      setAppLanguage('zh');
      await i18n.changeLanguage('zh');
    });

    await waitFor(() => {
      const postSwitchCalls = calls.slice(countBeforeLangSwitch);
      expect(postSwitchCalls.some((c) => c.language === 'zh' && c.page === 1)).toBe(true);
    });
    expect(calls.slice(countBeforeLangSwitch).some((c) => c.page === 2 && c.language === 'zh')).toBe(
      false,
    );
  });

  it('keeps Previous pagination accessible on an empty page > 1', async () => {
    const onSearch = vi.fn(async (query: SearchQuery): Promise<SearchResponse> => {
      const page = query.page ?? 1;
      return {
        items: page === 1 ? [sampleRecord(101)] : [],
        total: 48,
        page,
        pageSize: 24,
        source: 'catalog_server',
      };
    });

    renderWorkshop(onSearch);
    await screen.findByText('Campaign 101');

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await screen.findByText('No matches');

    const prevBtn = screen.getByRole('button', { name: 'Previous' });
    expect(prevBtn.hasAttribute('disabled')).toBe(false);
    fireEvent.click(prevBtn);

    await screen.findByText('Campaign 101');
  });

  it('disables pagination while loading and ignores out-of-order stale responses', async () => {
    const resolvers: Array<(res: SearchResponse) => void> = [];
    const onSearch = vi.fn(
      () =>
        new Promise<SearchResponse>((resolve) => {
          resolvers.push(resolve);
        }),
    );

    renderWorkshop(onSearch);
    await waitFor(() => expect(resolvers.length).toBe(1));
    await act(async () => {
      resolvers[0]({
        items: [sampleRecord(1)],
        total: 48,
        page: 1,
        pageSize: 24,
        source: 'catalog_server',
      });
    });
    await screen.findByText('Campaign 1');

    // Start page 2 request (stays pending)
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() => expect(resolvers.length).toBe(2));
    expect(screen.getByRole('button', { name: 'Next' }).hasAttribute('disabled')).toBe(true);
    expect(screen.getByRole('button', { name: 'Previous' }).hasAttribute('disabled')).toBe(true);
    expect(screen.getByText('1 / 2')).toBeTruthy();

    // Submit a new query before page 2 finishes; resolve newer query first, then stale page 2.
    const input = screen.getByPlaceholderText('Search Workshop');
    fireEvent.change(input, { target: { value: 'yama' } });
    fireEvent.submit(input.closest('form')!);
    await waitFor(() => expect(resolvers.length).toBe(3));

    await act(async () => {
      resolvers[2]({
        items: [sampleRecord(999)],
        total: 1,
        page: 1,
        pageSize: 24,
        source: 'catalog_server',
      });
    });
    await screen.findByText('Campaign 999');

    await act(async () => {
      resolvers[1]({
        items: [sampleRecord(2)],
        total: 48,
        page: 2,
        pageSize: 24,
        source: 'catalog_server',
      });
    });
    expect(screen.getByText('Campaign 999')).toBeTruthy();
    expect(screen.queryByText('Campaign 2')).toBeNull();
  });

  it('shows error details with Retry for transient errors and hides Retry for unsupported', async () => {
    let attempt = 0;
    const onSearch = vi.fn(async (): Promise<SearchResponse> => {
      attempt += 1;
      if (attempt === 1) {
        throw new IpcError('network', 'Catalog server returned HTTP 502');
      }
      if (attempt === 2) {
        return {
          items: [sampleRecord(202)],
          total: 1,
          page: 1,
          pageSize: 24,
          source: 'catalog_server',
        };
      }
      throw new IpcError('unsupported', 'Current source does not support search');
    });

    renderWorkshop(onSearch);
    await screen.findByText('Catalog server returned HTTP 502');
    const retryBtn = screen.getByRole('button', { name: 'Retry' });
    fireEvent.click(retryBtn);
    await screen.findByText('Campaign 202');

    cleanup();
    renderWorkshop(onSearch);
    await screen.findByText('Search unavailable');
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  it('lets users switch to collection search and resets pagination to page 1', async () => {
    const calls: SearchQuery[] = [];
    const onTrackCollection = vi.fn(async () => null);
    const onSearch = vi.fn(async (query: SearchQuery): Promise<SearchResponse> => {
      calls.push(query);
      if (query.fileType === 'collection') {
        return {
          items: [
            {
              workshopId: '3499001245',
              fileType: 'collection',
              status: 'active',
              title: 'Friday Coop Essential Pack',
              description: 'Curated co-op campaigns.',
              tags: ['Campaigns'],
              previews: [],
              dependencies: [],
              collectionChildren: ['2814104912'],
              source: 'catalog_server',
              fetchedAt: '2026-11-24T00:00:00Z',
            },
          ],
          total: 1,
          page: query.page ?? 1,
          pageSize: 24,
          source: 'catalog_server',
        };
      }
      return {
        items: [sampleRecord(query.page ?? 1)],
        total: 48,
        page: query.page ?? 1,
        pageSize: 24,
        source: 'catalog_server',
      };
    });

    render(
      <WorkshopView
        addons={[]}
        collections={[]}
        liveDownloads={{}}
        onSearch={onSearch}
        onFetchItem={async () => null}
        onImportSubscribed={async () => null}
        onDownload={async () => null}
        onUpdate={async () => null}
        onCancelDownload={() => {}}
        onTrackCollection={onTrackCollection}
        onOpenSourcesSettings={() => {}}
        onOpenExternal={() => {}}
        onInvalidId={() => {}}
        onNotify={() => {}}
      />,
    );

    await screen.findByText('Campaign 1');
    expect(calls[0].fileType).toBeUndefined();

    // Advance to page 2 on Items, then switch to Collection
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await screen.findByText('Campaign 2');

    const contentGroup = screen.getByRole('group', { name: 'Content type' });
    const collectionBtn = Array.from(contentGroup.querySelectorAll('button')).find(
      (btn) => btn.textContent === 'Collection',
    )!;
    fireEvent.click(collectionBtn);

    await screen.findByText('Friday Coop Essential Pack');
    expect(calls.at(-1)).toMatchObject({
      fileType: 'collection',
      page: 1,
    });

    // Switch back to Item and verify fileType omits explicit filter so bare collection IDs still work
    const itemBtn = Array.from(contentGroup.querySelectorAll('button')).find(
      (btn) => btn.textContent === 'Item',
    )!;
    fireEvent.click(itemBtn);
    await screen.findByText('Campaign 1');
    expect(calls.at(-1)?.fileType).toBeUndefined();
    expect(calls.at(-1)?.page).toBe(1);
  });

  it('discovers collections by name when fileType=collection in preview Workshop search and honors title-only numeric queries', async () => {
    const { searchPreviewWorkshop } = await import('../../api/preview/workshop');

    // Default (no fileType) excludes collections on name search, while fileType=collection finds them
    const defaultRes = searchPreviewWorkshop({ query: 'Friday Coop' });
    expect(defaultRes.items.map((i) => i.workshopId)).toEqual([]);

    const collectionRes = searchPreviewWorkshop({
      query: 'Friday Coop',
      fileType: 'collection',
    });
    expect(collectionRes.items.map((i) => i.workshopId)).toEqual(['3499001245']);

    // Bare 6+ digit collection ID remains searchable without explicit fileType, unless fileType=item
    expect(searchPreviewWorkshop({ query: '3499001245' }).items.map((i) => i.workshopId)).toEqual([
      '3499001245',
    ]);
    expect(
      searchPreviewWorkshop({ query: '3499001245', fileType: 'item' }).items.map(
        (i) => i.workshopId,
      ),
    ).toEqual([]);

    // Title-only numeric search does not match by Workshop ID when title lacks the number, while direct URL still works
    expect(
      searchPreviewWorkshop({ query: '2814104912', searchTarget: 'title' }).items,
    ).toHaveLength(0);
    expect(
      searchPreviewWorkshop({
        query: 'https://steamcommunity.com/sharedfiles/filedetails/?id=2814104912',
        searchTarget: 'title',
      }).items.map((i) => i.workshopId),
    ).toEqual(['2814104912']);
  });
});
