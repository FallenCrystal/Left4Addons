import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { TopBar } from './TopBar';

describe('TopBar', () => {
  const categoriesList = ['All', 'Campaign', 'Survivor'];

  test('renders search query, categories, sort select, and action buttons', () => {
    const onSearchQueryChange = vi.fn();
    const onCategoryChange = vi.fn();
    const onSortByChange = vi.fn();
    const onToggleSelectMode = vi.fn();
    const onSyncSteam = vi.fn();

    render(
      <TopBar
        searchQuery="tank"
        onSearchQueryChange={onSearchQueryChange}
        selectedCategory="All"
        onCategoryChange={onCategoryChange}
        sortBy="size"
        onSortByChange={onSortByChange}
        syncingSteam={false}
        onSyncSteam={onSyncSteam}
        categoriesList={categoriesList}
        isSelectMode={false}
        onToggleSelectMode={onToggleSelectMode}
      />
    );

    // Search query input
    const searchInput = screen.getByPlaceholderText('名称, 描述, 作者或创意工坊ID') as HTMLInputElement;
    expect(searchInput.value).toBe('tank');

    // Search query input change
    fireEvent.change(searchInput, { target: { value: 'witch' } });
    expect(onSearchQueryChange).toHaveBeenCalledWith('witch');

    // Category list render
    expect(screen.getByText('全部')).toBeDefined();
    expect(screen.getByText('战役')).toBeDefined();
    expect(screen.getByText('幸存者')).toBeDefined();

    // Clicking category
    fireEvent.click(screen.getByText('战役'));
    expect(onCategoryChange).toHaveBeenCalledWith('Campaign');

    // Sort select trigger
    const selectTrigger = screen.getByText('大小');
    expect(selectTrigger).toBeDefined();

    // Open dropdown
    fireEvent.click(selectTrigger);

    // Click option
    const titleOption = screen.getByText('名称');
    fireEvent.click(titleOption);
    expect(onSortByChange).toHaveBeenCalledWith('title');

    // Batch Manage button
    expect(screen.getByText('批量管理')).toBeDefined();
    fireEvent.click(screen.getByText('批量管理'));
    expect(onToggleSelectMode).toHaveBeenCalledTimes(1);

    // Sync button
    const buttons = screen.getAllByRole('button');
    const syncButton = buttons[buttons.length - 1]; // last button is Sync Steam icon-only
    fireEvent.click(syncButton);
    expect(onSyncSteam).toHaveBeenCalledTimes(1);
  });

  test('renders batch management correctly in active select mode', () => {
    const onToggleSelectMode = vi.fn();

    render(
      <TopBar
        searchQuery=""
        onSearchQueryChange={vi.fn()}
        selectedCategory="All"
        onCategoryChange={vi.fn()}
        sortBy="title"
        onSortByChange={vi.fn()}
        syncingSteam={false}
        onSyncSteam={vi.fn()}
        categoriesList={categoriesList}
        isSelectMode={true}
        onToggleSelectMode={onToggleSelectMode}
      />
    );

    expect(screen.getByText('退出批量')).toBeDefined();
    fireEvent.click(screen.getByText('退出批量'));
    expect(onToggleSelectMode).toHaveBeenCalledTimes(1);
  });
});
