import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { GroupModal } from './GroupModal';
import { Addon } from '../types/addon';

describe('GroupModal', () => {
  const mockAddons: Record<string, Addon> = {
    'addon1.vpk': {
      vpkName: 'addon1.vpk',
      dirType: 'loading',
      isEnabled: true,
      fileSize: 100,
      filesCount: 1,
      steamDetails: { title: 'First Addon' },
    },
    'addon2.vpk': {
      vpkName: 'addon2.vpk',
      dirType: 'loading',
      isEnabled: true,
      fileSize: 200,
      filesCount: 2,
      addonInfo: { addontitle: 'Second Addon' },
    },
  };

  test('renders nothing when open is false', () => {
    const { container } = render(
      <GroupModal
        open={false}
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders fields and handles checkbox selection', () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();

    render(
      <GroupModal
        open={true}
        addons={mockAddons}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    );

    expect(screen.getByText('创建新附件分组')).toBeDefined();
    
    // Renders the checkboxes
    expect(screen.getByText('First Addon')).toBeDefined();
    expect(screen.getByText('Second Addon')).toBeDefined();

    // Check cancel button
    fireEvent.click(screen.getByText('取消'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  test('submitting forms with inputs triggers onConfirm', () => {
    const onConfirm = vi.fn();

    render(
      <GroupModal
        open={true}
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const nameInput = screen.getByPlaceholderText('例如：Early Days 战役地图包') as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: 'My Campaign Pack' } });

    // Select first addon
    const firstCheckbox = screen.getAllByRole('checkbox')[0] as HTMLInputElement;
    fireEvent.click(firstCheckbox);
    expect(firstCheckbox.checked).toBe(true);

    const form = nameInput.closest('form');
    if (form) {
      fireEvent.submit(form);
      expect(onConfirm).toHaveBeenCalledWith('My Campaign Pack', ['addon1.vpk']);
    }
  });

  test('submits button is disabled if fields are incomplete', () => {
    render(
      <GroupModal
        open={true}
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );

    const submitButton = screen.getByText('创建分组') as HTMLButtonElement;
    expect(submitButton.disabled).toBe(true);

    const nameInput = screen.getByPlaceholderText('例如：Early Days 战役地图包') as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: 'My Campaign Pack' } });
    // Still disabled because no addons are selected
    expect(submitButton.disabled).toBe(true);

    // Select first addon
    const firstCheckbox = screen.getAllByRole('checkbox')[0] as HTMLInputElement;
    fireEvent.click(firstCheckbox);
    expect(submitButton.disabled).toBe(false);
  });
});
