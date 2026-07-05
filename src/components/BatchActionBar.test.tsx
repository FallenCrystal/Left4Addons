import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { BatchActionBar } from './BatchActionBar';
import { Addon, Group } from '../types/addon';

describe('BatchActionBar', () => {
  const mockAddons: Record<string, Addon> = {
    'addon1.vpk': { vpkName: 'addon1.vpk', dirType: 'loading', isEnabled: true, fileSize: 100, filesCount: 1 },
    'addon2.vpk': { vpkName: 'addon2.vpk', dirType: 'workshop', isEnabled: false, fileSize: 200, filesCount: 2 },
  };

  const mockGroups: Group[] = [
    { id: 'group-1', name: 'Group 1', addons: [] },
  ];

  const filteredItems = Object.values(mockAddons);

  test('renders batch actions and fires callbacks', () => {
    const onSelectAll = vi.fn();
    const onBatchToggle = vi.fn();
    const onBatchMove = vi.fn();
    const onBatchRename = vi.fn();
    const onBatchAddToGroup = vi.fn();
    const onClearSelection = vi.fn();

    render(
      <BatchActionBar
        selectedVpkNames={['addon1.vpk']}
        filteredItems={filteredItems}
        addons={mockAddons}
        groups={mockGroups}
        onSelectAll={onSelectAll}
        onBatchToggle={onBatchToggle}
        onBatchMove={onBatchMove}
        onBatchRename={onBatchRename}
        onBatchAddToGroup={onBatchAddToGroup}
        onClearSelection={onClearSelection}
      />
    );

    // Verify information text
    expect(screen.getByText('已选择 1 个组件')).toBeDefined();

    // Trigger select all
    fireEvent.click(screen.getByText('全选'));
    expect(onSelectAll).toHaveBeenCalledWith(filteredItems);

    // Trigger batch enable
    fireEvent.click(screen.getByText('启用'));
    expect(onBatchToggle).toHaveBeenCalledWith(true);

    // Trigger batch disable
    fireEvent.click(screen.getByText('禁用'));
    expect(onBatchToggle).toHaveBeenCalledWith(false);

    // Trigger batch rename
    fireEvent.click(screen.getByText('自动重命名'));
    expect(onBatchRename).toHaveBeenCalledTimes(1);

    // Trigger clear selection / exit batch mode
    fireEvent.click(screen.getByText('退出批量'));
    expect(onClearSelection).toHaveBeenCalledTimes(1);
  });

  test('shows move to loading button if a workshop addon is selected', () => {
    const onBatchMove = vi.fn();

    render(
      <BatchActionBar
        selectedVpkNames={['addon2.vpk']} // addon2 is workshop
        filteredItems={filteredItems}
        addons={mockAddons}
        groups={mockGroups}
        onSelectAll={vi.fn()}
        onBatchToggle={vi.fn()}
        onBatchMove={onBatchMove}
        onBatchRename={vi.fn()}
        onBatchAddToGroup={vi.fn()}
        onClearSelection={vi.fn()}
      />
    );

    // The move button should be visible
    const moveBtn = screen.getByText('移动至手动安装');
    expect(moveBtn).toBeDefined();

    fireEvent.click(moveBtn);
    expect(onBatchMove).toHaveBeenCalledTimes(1);
  });
});
