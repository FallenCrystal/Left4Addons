import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { GroupModal } from './GroupModal';

describe('GroupModal', () => {
  test('renders nothing when open is false', () => {
    const { container } = render(
      <GroupModal
        open={false}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders name input and handles cancel', () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();

    render(
      <GroupModal
        open={true}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    );

    expect(screen.getByText('创建新附件分组')).toBeDefined();
    expect(screen.getByPlaceholderText('例如：Early Days 战役地图包')).toBeDefined();

    // Check cancel button
    fireEvent.click(screen.getByText('取消'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  test('submitting form with name triggers onConfirm', () => {
    const onConfirm = vi.fn();

    render(
      <GroupModal
        open={true}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const nameInput = screen.getByPlaceholderText('例如：Early Days 战役地图包') as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: 'My Campaign Pack' } });

    const form = nameInput.closest('form');
    if (form) {
      fireEvent.submit(form);
      expect(onConfirm).toHaveBeenCalledWith('My Campaign Pack');
    }
  });

  test('submit button is disabled if name is empty', () => {
    render(
      <GroupModal
        open={true}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );

    const submitButton = screen.getByText('创建分组') as HTMLButtonElement;
    expect(submitButton.disabled).toBe(true);

    const nameInput = screen.getByPlaceholderText('例如：Early Days 战役地图包') as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: 'My Campaign Pack' } });
    expect(submitButton.disabled).toBe(false);
  });
});
