import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { RenameModal } from './RenameModal';
import { Addon, Group } from '../types/addon';

describe('RenameModal', () => {
  const mockAddon: Addon = {
    vpkName: 'old_name.vpk',
    dirType: 'loading',
    isEnabled: true,
    fileSize: 100,
    filesCount: 1,
    workshopId: '12345',
    steamDetails: { title: 'My Awesome Addon' },
  };

  const mockGroup: Group = {
    id: 'g1',
    name: 'Campaign Group',
    addons: ['old_name.vpk'],
  };

  const mockAddons: Record<string, Addon> = {
    'old_name.vpk': mockAddon,
  };

  test('renders nothing when open is false', () => {
    const { container } = render(
      <RenameModal
        open={false}
        currentName="old_name.vpk"
        title="My Awesome Addon"
        suggestedName="new_name.vpk"
        addon={mockAddon}
        itemGroup={mockGroup}
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders current name, input field and action buttons when open is true', () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();

    render(
      <RenameModal
        open={true}
        currentName="old_name.vpk"
        title="My Awesome Addon"
        suggestedName="new_name.vpk"
        addon={mockAddon}
        itemGroup={mockGroup}
        addons={mockAddons}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    );

    expect(screen.getByText('重命名附加组件文件')).toBeDefined();
    expect(screen.getByText('old_name.vpk')).toBeDefined();
    
    const input = screen.getByRole('textbox') as HTMLInputElement;
    expect(input.value).toBe('new_name.vpk');

    expect(screen.getByText('取消')).toBeDefined();
    expect(screen.getByText('重命名')).toBeDefined();
  });

  test('clicking cancel calls onCancel', () => {
    const onCancel = vi.fn();

    render(
      <RenameModal
        open={true}
        currentName="old_name.vpk"
        title="My Awesome Addon"
        suggestedName="new_name.vpk"
        addon={mockAddon}
        itemGroup={mockGroup}
        addons={mockAddons}
        onCancel={onCancel}
        onConfirm={vi.fn()}
      />
    );

    fireEvent.click(screen.getByText('取消'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  test('submitting form calls onConfirm with input value', () => {
    const onConfirm = vi.fn();

    render(
      <RenameModal
        open={true}
        currentName="old_name.vpk"
        title="My Awesome Addon"
        suggestedName="new_name.vpk"
        addon={mockAddon}
        itemGroup={mockGroup}
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const input = screen.getByRole('textbox') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'custom_name.vpk' } });

    const form = input.closest('form');
    if (form) {
      fireEvent.submit(form);
      expect(onConfirm).toHaveBeenCalledWith('old_name.vpk', 'custom_name.vpk');
    }
  });

  test('clicking apply workshop title updates input to suggestion from helper', () => {
    render(
      <RenameModal
        open={true}
        currentName="old_name.vpk"
        title="My Awesome Addon"
        suggestedName="new_name.vpk"
        addon={mockAddon}
        itemGroup={mockGroup}
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );

    const applyButton = screen.getByText('应用创意工坊标题作为文件名');
    fireEvent.click(applyButton);

    const input = screen.getByRole('textbox') as HTMLInputElement;
    // getSuggestedVpkName should return '[12345][Campaign Group]My Awesome Addon.vpk'
    expect(input.value).toBe('[12345][Campaign Group]My Awesome Addon.vpk');
  });
});
