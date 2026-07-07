import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { GroupCard } from './GroupCard';
import { Addon, Group } from '../types/addon';

describe('GroupCard', () => {
  const mockGroup: Group = {
    id: 'group-1',
    name: 'My Group',
    addons: ['addon1.vpk', 'addon2.vpk'],
    tags: ['Map', 'Skin'],
  };

  const mockAddons: Addon[] = [
    {
      id: 'addon1.vpk', vpkName: 'addon1.vpk',
      dirType: 'loading',
      isEnabled: true,
      fileSize: 1024 * 1024 * 10, // 10 MB
      filesCount: 5,
      addonInfo: {
        addonAuthor: 'Author A',
        addontitle: 'Addon One',
      },
    },
    {
      id: 'addon2.vpk', vpkName: 'addon2.vpk',
      dirType: 'loading',
      isEnabled: true,
      fileSize: 1024 * 1024 * 5, // 5 MB
      filesCount: 3,
      addonInfo: {
        addonAuthor: 'Author A',
        addontitle: 'Addon Two',
      },
    },
  ];

  test('renders group information correctly', () => {
    const onToggleGroup = vi.fn();
    const onViewGroupDetails = vi.fn();

    render(
      <GroupCard
        group={mockGroup}
        addons={mockAddons}
        onToggleGroup={onToggleGroup}
        onViewGroupDetails={onViewGroupDetails}
      />
    );

    // Group name
    expect(screen.getByText('My Group')).toBeDefined();
    // Tags
    expect(screen.getByText('Map')).toBeDefined();
    expect(screen.getByText('Skin')).toBeDefined();
    // Author (single author for both)
    expect(screen.getByText('作者: Author A')).toBeDefined();
    // Size formatted (15 MB)
    expect(screen.getByText('总大小: 15 MB')).toBeDefined();
    // Enabled status badge
    expect(screen.getByText('已启用')).toBeDefined();
  });

  test('renders partially enabled status and multiple authors correctly', () => {
    const mixedAddons = [
      { ...mockAddons[0], isEnabled: true, addonInfo: { addonAuthor: 'Author A' } },
      { ...mockAddons[1], isEnabled: false, addonInfo: { addonAuthor: 'Author B' } },
    ];

    render(
      <GroupCard
        group={mockGroup}
        addons={mixedAddons}
        onToggleGroup={vi.fn()}
        onViewGroupDetails={vi.fn()}
      />
    );

    expect(screen.getByText('部分启用')).toBeDefined();
    expect(screen.getByText('作者: Author A 等 1 位作者')).toBeDefined();
  });

  test('renders disabled status when all addons disabled', () => {
    const disabledAddons = [
      { ...mockAddons[0], isEnabled: false },
      { ...mockAddons[1], isEnabled: false },
    ];

    render(
      <GroupCard
        group={mockGroup}
        addons={disabledAddons}
        onToggleGroup={vi.fn()}
        onViewGroupDetails={vi.fn()}
      />
    );

    expect(screen.getByText('已禁用')).toBeDefined();
  });

  test('calls onViewGroupDetails when card is clicked and select mode is false', () => {
    const onViewGroupDetails = vi.fn();
    render(
      <GroupCard
        group={mockGroup}
        addons={mockAddons}
        onToggleGroup={vi.fn()}
        onViewGroupDetails={onViewGroupDetails}
      />
    );

    // Click the clickable area
    const clickable = screen.getByText('My Group');
    fireEvent.click(clickable);
    expect(onViewGroupDetails).toHaveBeenCalledWith('group-1');
  });

  test('calls onSelectGroupToggle when card is clicked and select mode is true', () => {
    const onSelectGroupToggle = vi.fn();
    render(
      <GroupCard
        group={mockGroup}
        addons={mockAddons}
        onToggleGroup={vi.fn()}
        onViewGroupDetails={vi.fn()}
        isSelectMode={true}
        onSelectGroupToggle={onSelectGroupToggle}
      />
    );

    // Click the clickable area
    const clickable = screen.getByText('My Group');
    fireEvent.click(clickable);
    expect(onSelectGroupToggle).toHaveBeenCalledWith(mockAddons, 'group-1');
  });

  test('calls onToggleGroup when the switch is toggled', () => {
    const onToggleGroup = vi.fn();
    render(
      <GroupCard
        group={mockGroup}
        addons={mockAddons}
        onToggleGroup={onToggleGroup}
        onViewGroupDetails={vi.fn()}
      />
    );

    const checkbox = screen.getByRole('checkbox');
    fireEvent.click(checkbox);
    // Since mockAddons are allEnabled=true, clicking the checkbox triggers onChange which does onToggleGroup(addons, false)
    expect(onToggleGroup).toHaveBeenCalledWith(mockAddons, false);
  });
});
