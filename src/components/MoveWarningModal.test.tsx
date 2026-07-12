import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { MoveWarningModal } from './MoveWarningModal';

describe('MoveWarningModal', () => {
  test('renders nothing when open is false', () => {
    const { container } = render(
      <MoveWarningModal
        open={false}
        id="test.vpk"
        currentDirType="workshop"
        workshopId="12345"
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders warning messages and action buttons when open is true and workshopId is provided', () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();

    render(
      <MoveWarningModal
        open={true}
        id="test.vpk"
        currentDirType="workshop"
        workshopId="12345"
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    );

    expect(screen.getByText('移动创意工坊附件提示')).toBeDefined();
    expect(screen.getByText('非常重要：')).toBeDefined();
    expect(screen.getByText('取消')).toBeDefined();
    expect(screen.getByText('取消订阅并移动')).toBeDefined();
    expect(screen.getByText('直接移动')).toBeDefined();
  });

  test('does not render unsubscribe button when workshopId is empty', () => {
    render(
      <MoveWarningModal
        open={true}
        id="test.vpk"
        currentDirType="loading"
        workshopId=""
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );

    expect(screen.queryByText('取消订阅并移动')).toBeNull();
    expect(screen.getByText('直接移动')).toBeDefined();
  });

  test('calls onCancel when cancel is clicked', () => {
    const onCancel = vi.fn();

    render(
      <MoveWarningModal
        open={true}
        id="test.vpk"
        currentDirType="workshop"
        workshopId="12345"
        onCancel={onCancel}
        onConfirm={vi.fn()}
      />
    );

    fireEvent.click(screen.getByText('取消'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  test('calls onConfirm with unsubscribe=true when unsubscribe and move is clicked', () => {
    const onConfirm = vi.fn();

    render(
      <MoveWarningModal
        open={true}
        id="test.vpk"
        currentDirType="workshop"
        workshopId="12345"
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('取消订阅并移动'));
    expect(onConfirm).toHaveBeenCalledWith('test.vpk', 'workshop', true);
  });

  test('calls onConfirm with unsubscribe=false when move directly is clicked', () => {
    const onConfirm = vi.fn();

    render(
      <MoveWarningModal
        open={true}
        id="test.vpk"
        currentDirType="workshop"
        workshopId="12345"
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('直接移动'));
    expect(onConfirm).toHaveBeenCalledWith('test.vpk', 'workshop', false);
  });
});
