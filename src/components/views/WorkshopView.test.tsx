import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import '../../i18n';
import { setAppLanguage } from '../../i18n';
import type { SearchQuery, SearchResponse, WorkshopItemRecord } from '../../api/ipc';
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

describe('WorkshopView', () => {
  beforeEach(() => {
    setAppLanguage('en');
  });

  afterEach(() => {
    setAppLanguage('en');
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
        onTrackCollection={async () => null}
        onOpenSourcesSettings={() => {}}
        onOpenExternal={() => {}}
        onInvalidId={() => {}}
        onNotify={() => {}}
      />,
    );

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
    });

    await waitFor(() => {
      const postSwitchCalls = calls.slice(countBeforeLangSwitch);
      expect(postSwitchCalls.some((c) => c.language === 'zh' && c.page === 1)).toBe(true);
    });
    expect(calls.slice(countBeforeLangSwitch).some((c) => c.page === 2 && c.language === 'zh')).toBe(
      false,
    );
  });
});
