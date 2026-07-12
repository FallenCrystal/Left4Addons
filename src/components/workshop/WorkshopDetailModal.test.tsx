import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';
import { WorkshopDetailModal } from './WorkshopDetailModal';
import type { WorkshopItem, WorkshopPageDetails } from './types';

const {
  mockFetchWorkshopPageDetails,
  mockGetWorkshopPageSnapshot,
  mockPersistWorkshopPageDetails,
} = vi.hoisted(() => ({
  mockFetchWorkshopPageDetails: vi.fn(),
  mockGetWorkshopPageSnapshot: vi.fn(),
  mockPersistWorkshopPageDetails: vi.fn(),
}));

vi.mock('../../services/workshopClient', () => ({
  fetchWorkshopPageDetails: (...args: unknown[]) => mockFetchWorkshopPageDetails(...args),
  getWorkshopPageSnapshot: (...args: unknown[]) => mockGetWorkshopPageSnapshot(...args),
  persistWorkshopPageDetails: (...args: unknown[]) => mockPersistWorkshopPageDetails(...args),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

function createWorkshopItem(): WorkshopItem {
  return {
    workshopId: '3560883926',
    title: 'Early Days PART 1/6',
    imagePath: '',
    authorName: 'perfect_buddy',
    authorId: '76561198012020581',
    authorUrl: 'https://steamcommunity.com/id/perfectbuddy',
    stars: 5,
    shortDescription: 'Subscribe to ALL 6 PART',
  };
}

function createPageDetails(): WorkshopPageDetails {
  return {
    title: 'Early Days PART 1/6',
    description: 'Subscribe to ALL 6 PARTS.\n\nPlease rate part 1.',
    imageGallery: [],
    tags: [],
    requiredItems: [],
    parentCollections: [],
  };
}

describe('WorkshopDetailModal', () => {
  beforeEach(() => {
    mockFetchWorkshopPageDetails.mockReset();
    mockGetWorkshopPageSnapshot.mockReset();
    mockPersistWorkshopPageDetails.mockReset();
    mockPersistWorkshopPageDetails.mockResolvedValue({});
  });

  test('prefers full page description over item short description', async () => {
    mockGetWorkshopPageSnapshot.mockResolvedValue(createPageDetails());
    mockFetchWorkshopPageDetails.mockResolvedValue(createPageDetails());

    const { container } = render(
      <WorkshopDetailModal
        open
        item={createWorkshopItem()}
        collection={null}
        onClose={vi.fn()}
        onDownload={vi.fn()}
        onOpenLink={vi.fn()}
        onImportCollection={vi.fn()}
        onItemNavigate={vi.fn()}
        onCollectionNavigate={vi.fn()}
        addons={{}}
        knownUninstalledAddons={{}}
        downloadProgress={{}}
        isSubmitting={false}
        groups={[]}
        isLoading={false}
        onDatabaseUpdate={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(mockFetchWorkshopPageDetails).toHaveBeenCalledWith('3560883926', 'workshop-detail');
    });

    const description = container.querySelector('.description-block')?.textContent;
    expect(description).toBe('Subscribe to ALL 6 PARTS.\n\nPlease rate part 1.');
    expect(description).not.toBe('Subscribe to ALL 6 PART');
  });

  test('uses the scraped cover for a no-gallery item and its download task', async () => {
    const pageDetails = {
      ...createPageDetails(),
      previewUrl: 'https://images.steamusercontent.com/ugc/cover-image/',
    };
    const onDownload = vi.fn();
    mockGetWorkshopPageSnapshot.mockResolvedValue(pageDetails);
    mockFetchWorkshopPageDetails.mockResolvedValue(pageDetails);

    render(
      <WorkshopDetailModal
        open
        item={createWorkshopItem()}
        collection={null}
        onClose={vi.fn()}
        onDownload={onDownload}
        onOpenLink={vi.fn()}
        onImportCollection={vi.fn()}
        onItemNavigate={vi.fn()}
        onCollectionNavigate={vi.fn()}
        addons={{}}
        knownUninstalledAddons={{}}
        downloadProgress={{}}
        isSubmitting={false}
        groups={[]}
        isLoading={false}
        onDatabaseUpdate={vi.fn()}
      />,
    );

    const cover = await screen.findByAltText('Early Days PART 1/6') as HTMLImageElement;
    expect(cover.src).toBe('https://images.steamusercontent.com/ugc/cover-image/');

    fireEvent.click(screen.getByRole('button', { name: /下载/i }));
    expect(onDownload).toHaveBeenCalledWith(
      '3560883926',
      'Early Days PART 1/6',
      'https://images.steamusercontent.com/ugc/cover-image/',
    );
  });
});
