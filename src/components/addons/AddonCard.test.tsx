import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { AddonCard } from './AddonCard';
import { Addon } from '../../types/addon';

describe('AddonCard', () => {
  const mockInstalledAddon: Addon = {
    id: 'addon1.vpk',
    vpkName: 'addon1.vpk',
    dirType: 'loading',
    isEnabled: true,
    fileSize: 1024 * 1024,
    filesCount: 1,
    currentPath: '/home/user/game/addons/addon1.vpk',
  };

  const mockUninstalledAddon: Addon = {
    id: '12345',
    vpkName: '12345.vpk',
    dirType: 'none',
    isEnabled: false,
    fileSize: 0,
    filesCount: 0,
    workshopId: '12345',
  };

  test('renders open in file manager button for installed addon and triggers callback', () => {
    const onOpenInFileManager = vi.fn();
    render(
      <AddonCard
        addon={mockInstalledAddon}
        groups={[]}
        onToggle={vi.fn()}
        onAddToGroup={vi.fn()}
        onOpenLink={vi.fn()}
        onOpenInFileManager={onOpenInFileManager}
        onMoveClick={vi.fn()}
        onRenameClick={vi.fn()}
        onDetailClick={vi.fn()}
        isSelectMode={false}
        isSelected={false}
        onSelectToggle={vi.fn()}
      />
    );

    const openBtn = screen.getByTitle('在文件管理器中打开');
    expect(openBtn).toBeDefined();
    fireEvent.click(openBtn);
    expect(onOpenInFileManager).toHaveBeenCalledWith('/home/user/game/addons/addon1.vpk');
  });

  test('does not render open in file manager button for uninstalled addon', () => {
    const onOpenInFileManager = vi.fn();
    render(
      <AddonCard
        addon={mockUninstalledAddon}
        groups={[]}
        onToggle={vi.fn()}
        onAddToGroup={vi.fn()}
        onOpenLink={vi.fn()}
        onOpenInFileManager={onOpenInFileManager}
        onMoveClick={vi.fn()}
        onRenameClick={vi.fn()}
        onDetailClick={vi.fn()}
        isSelectMode={false}
        isSelected={false}
        onSelectToggle={vi.fn()}
      />
    );

    expect(screen.queryByTitle('在文件管理器中打开')).toBeNull();
  });
});
