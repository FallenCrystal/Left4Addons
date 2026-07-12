import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { GroupHeader } from './GroupHeader';
import { Addon, Group } from '../types/addon';

describe('GroupHeader', () => {
  const mockGroup: Group = {
    id: 'group-abc',
    name: 'Campaign Maps',
    addons: ['addon1.vpk', 'addon2.vpk'],
  };

  const mockAddons: Addon[] = [
    {
      id: 'addon1.vpk', vpkName: 'addon1.vpk',
      dirType: 'loading', isEnabled: true, fileSize: 100, filesCount: 1,
    },
    {
      id: 'addon2.vpk', vpkName: 'addon2.vpk',
      dirType: 'workshop', isEnabled: false, fileSize: 200, filesCount: 2,
    },
  ];

  test('renders group details and handles actions', () => {
    const onRenameGroup = vi.fn();
    const onDeleteGroup = vi.fn();
    const onGroupActionBatch = vi.fn();

    render(
      <GroupHeader
        currentGroup={mockGroup}
        groupAddons={mockAddons}
        onRenameGroup={onRenameGroup}
        onDeleteGroup={onDeleteGroup}
        onGroupActionBatch={onGroupActionBatch}
      />
    );

    // Verify render details
    expect(screen.getByText('Campaign Maps')).toBeDefined();
    expect(screen.getByText(/此分组包含 2 个附件文件/)).toBeDefined();

    // Trigger Rename
    fireEvent.click(screen.getByText('重命名分组'));
    expect(onRenameGroup).toHaveBeenCalledTimes(1);

    // Trigger Delete (Dissolve)
    fireEvent.click(screen.getByText('解散分组'));
    expect(onDeleteGroup).toHaveBeenCalledTimes(1);

    // Trigger Enable all
    fireEvent.click(screen.getByText('全部启用'));
    expect(onGroupActionBatch).toHaveBeenCalledWith('enable');

    // Trigger Disable all
    fireEvent.click(screen.getByText('全部禁用'));
    expect(onGroupActionBatch).toHaveBeenCalledWith('disable');

    // Trigger Move all
    fireEvent.click(screen.getByText('全部移动至手动安装'));
    expect(onGroupActionBatch).toHaveBeenCalledWith('move-load');
  });
});
