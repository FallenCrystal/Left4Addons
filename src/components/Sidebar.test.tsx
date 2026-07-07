import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { Sidebar } from './Sidebar';
import { Addon, Group } from '../types/addon';

describe('Sidebar', () => {
  const mockAddons: Record<string, Addon> = {
    'addon1.vpk': {
      id: 'addon1.vpk', vpkName: 'addon1.vpk',
      dirType: 'loading',
      isEnabled: true,
      fileSize: 100,
      filesCount: 1,
    },
    'addon2.vpk': {
      id: 'addon2.vpk', vpkName: 'addon2.vpk',
      dirType: 'workshop',
      isEnabled: false,
      fileSize: 200,
      filesCount: 2,
    },
  };

  const mockGroups: Group[] = [
    {
      id: 'g1',
      name: 'Custom Group Alpha',
      addons: ['addon1.vpk'],
    },
  ];

  const defaultProps = {
    addons: mockAddons,
    groups: mockGroups,
    masterCollections: [],
    currentFilterTab: 'all',
    selectedGroupId: null,
    selectedMasterCollectionId: null,
    onFilterTabChange: vi.fn(),
    onOpenGroupModal: vi.fn(),
    onOpenMasterCollectionModal: vi.fn(),
    onAutoGrouping: vi.fn(),
    autoGrouping: false,
    disabledCount: 1,
    totalAddonsCount: 2,
    knownUninstalledCount: 3,
  };

  test('renders menu items and badges correctly', () => {
    render(<Sidebar {...defaultProps} />);

    expect(screen.getByText('全部组件')).toBeDefined();
    expect(screen.getByText('全部组件').closest('button')?.querySelector('.menu-item-badge')?.textContent).toBe('2');

    expect(screen.getByText('手动安装 (Addons)')).toBeDefined();
    expect(screen.getByText('手动安装 (Addons)').closest('button')?.querySelector('.menu-item-badge')?.textContent).toBe('1');

    expect(screen.getByText('创意工坊目录 (Workshop)')).toBeDefined();
    expect(screen.getByText('创意工坊目录 (Workshop)').closest('button')?.querySelector('.menu-item-badge')?.textContent).toBe('1');

    expect(screen.getByText('被禁用的组件 (.disabled)')).toBeDefined();
    expect(screen.getByText('被禁用的组件 (.disabled)').closest('button')?.querySelector('.menu-item-badge')?.textContent).toBe('1');

    expect(screen.getByText('未安装的已知组件')).toBeDefined();
    expect(screen.getByText('未安装的已知组件').closest('button')?.querySelector('.menu-item-badge')?.textContent).toBe('3');

    // Groups rendering
    expect(screen.getByText('Custom Group Alpha')).toBeDefined();
  });

  test('renders noGroups text when groups list is empty', () => {
    render(<Sidebar {...defaultProps} groups={[]} knownUninstalledCount={0} />);

    expect(screen.getByText('暂无分组，可自动归类或手动创建。')).toBeDefined();
  });

  test('calls onFilterTabChange when menu items are clicked', () => {
    const onFilterTabChange = vi.fn();

    render(<Sidebar {...defaultProps} onFilterTabChange={onFilterTabChange} knownUninstalledCount={0} />);

    fireEvent.click(screen.getByText('手动安装 (Addons)'));
    expect(onFilterTabChange).toHaveBeenCalledWith('loading', null);

    fireEvent.click(screen.getByText('Custom Group Alpha'));
    expect(onFilterTabChange).toHaveBeenCalledWith('groups', 'g1');
  });

  test('calls actions on button click', () => {
    const onOpenGroupModal = vi.fn();
    const onAutoGrouping = vi.fn();

    render(
      <Sidebar
        {...defaultProps}
        onOpenGroupModal={onOpenGroupModal}
        onAutoGrouping={onAutoGrouping}
        knownUninstalledCount={0}
      />
    );

    fireEvent.click(screen.getByText('新建手动分组'));
    expect(onOpenGroupModal).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByText('自动识别战役并归类'));
    expect(onAutoGrouping).toHaveBeenCalledTimes(1);
  });
});
