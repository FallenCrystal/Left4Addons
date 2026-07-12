import { act, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, test, vi, beforeEach } from 'vitest';
import { DetailModal } from './DetailModal';
import type { Addon } from '../types/addon';
import type { WorkshopPageDetails } from './workshop/types';

const {
  mockFetchWorkshopPageDetails,
  mockGetWorkshopPageSnapshot,
  mockPersistWorkshopPageDetails,
} = vi.hoisted(() => ({
  mockFetchWorkshopPageDetails: vi.fn(),
  mockGetWorkshopPageSnapshot: vi.fn(),
  mockPersistWorkshopPageDetails: vi.fn(),
}));

vi.mock('../services/workshopClient', () => ({
  fetchWorkshopPageDetails: (...args: unknown[]) => mockFetchWorkshopPageDetails(...args),
  getWorkshopPageSnapshot: (...args: unknown[]) => mockGetWorkshopPageSnapshot(...args),
  persistWorkshopPageDetails: (...args: unknown[]) => mockPersistWorkshopPageDetails(...args),
}));

function createAddon(id: string, workshopId: string): Addon {
  return {
    id,
    vpkName: `${id}.vpk`,
    workshopId,
    dirType: 'none',
    isEnabled: false,
    fileSize: 0,
    filesCount: 0,
  };
}

function createPageDetails(title: string): WorkshopPageDetails {
  return {
    title,
    imageGallery: [],
    tags: [],
    requiredItems: [],
    parentCollections: [],
  };
}

describe('DetailModal', () => {
  beforeEach(() => {
    mockFetchWorkshopPageDetails.mockReset();
    mockGetWorkshopPageSnapshot.mockReset();
    mockPersistWorkshopPageDetails.mockReset();
    mockGetWorkshopPageSnapshot.mockResolvedValue(null);
    mockPersistWorkshopPageDetails.mockResolvedValue({});
  });

  test('ignores stale workshop detail responses after navigating to another dependency item', async () => {
    let resolveFirst: ((value: WorkshopPageDetails) => void) | undefined;
    let resolveSecond: ((value: WorkshopPageDetails) => void) | undefined;

    mockFetchWorkshopPageDetails.mockImplementation((workshopId: string) => {
      if (workshopId === '100') {
        return new Promise<WorkshopPageDetails>((resolve) => {
          resolveFirst = resolve;
        });
      }
      if (workshopId === '200') {
        return new Promise<WorkshopPageDetails>((resolve) => {
          resolveSecond = resolve;
        });
      }
      throw new Error(`Unexpected workshop id: ${workshopId}`);
    });

    const baseProps = {
      open: true,
      groups: [],
      onCancel: vi.fn(),
      onToggle: vi.fn(),
      onMove: vi.fn(),
      onOpenLink: vi.fn(),
      addons: {},
      knownUninstalledAddons: {},
      onItemNavigate: vi.fn(),
      onDatabaseUpdate: vi.fn(),
      onDownload: vi.fn(),
      downloadProgress: {},
      isSubmitting: false,
    };

    const { rerender } = render(
      <DetailModal
        {...baseProps}
        addon={createAddon('first-addon', '100')}
      />,
    );

    await waitFor(() => {
      expect(mockFetchWorkshopPageDetails).toHaveBeenCalledWith('100', 'addon-detail');
    });

    rerender(
      <DetailModal
        {...baseProps}
        addon={createAddon('second-addon', '200')}
      />,
    );

    await waitFor(() => {
      expect(mockFetchWorkshopPageDetails).toHaveBeenCalledWith('200', 'addon-detail');
    });

    await act(async () => {
      resolveSecond?.(createPageDetails('Second dependency'));
    });

    expect(screen.getByText('Second dependency')).toBeDefined();

    await act(async () => {
      resolveFirst?.(createPageDetails('First dependency'));
    });

    expect(screen.queryByText('First dependency')).toBeNull();
    expect(screen.getByText('Second dependency')).toBeDefined();
  });
});
