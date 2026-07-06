import { describe, test, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { WorkshopActionWarningModal } from './WorkshopActionWarningModal';
import { Addon } from '../types/addon';

const mockInvoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mockInvoke(cmd, args),
}));

describe('WorkshopActionWarningModal', () => {
  beforeEach(() => {
    mockInvoke.mockClear();
  });

  const mockAddons: Addon[] = [
    {
      vpkName: 'addon1.vpk',
      dirType: 'workshop',
      isEnabled: true,
      fileSize: 100,
      filesCount: 1,
      workshopId: '12345678',
    },
  ];

  test('renders nothing when open is false', () => {
    const { container } = render(
      <WorkshopActionWarningModal
        open={false}
        actionName="Test Action"
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders warning content and buttons when open is true', () => {
    render(
      <WorkshopActionWarningModal
        open={true}
        actionName="Test Action"
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );

    expect(screen.getByText('创意工坊附件已移动')).toBeDefined();
    expect(screen.getByText('自动转移成功')).toBeDefined();
    expect(screen.getByText('当前会话不再提醒此警告，并静默执行移动')).toBeDefined();
    expect(screen.getByText('我知道了')).toBeDefined();
    expect(screen.getByText('前往创意工坊')).toBeDefined();
  });

  test('calls onConfirm with skipWarning=false when "我知道了" is clicked', () => {
    const onConfirm = vi.fn();
    render(
      <WorkshopActionWarningModal
        open={true}
        actionName="Test Action"
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('我知道了'));
    expect(onConfirm).toHaveBeenCalledWith(false);
  });

  test('toggles skipWarning and calls onConfirm with true', () => {
    const onConfirm = vi.fn();
    render(
      <WorkshopActionWarningModal
        open={true}
        actionName="Test Action"
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const checkbox = screen.getByRole('checkbox') as HTMLInputElement;
    fireEvent.click(checkbox);
    expect(checkbox.checked).toBe(true);

    fireEvent.click(screen.getByText('我知道了'));
    expect(onConfirm).toHaveBeenCalledWith(true);
  });

  test('calls handleOpenWorkshop which invokes open_url and confirms', async () => {
    const onConfirm = vi.fn();
    render(
      <WorkshopActionWarningModal
        open={true}
        actionName="Test Action"
        addons={mockAddons}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const button = screen.getByRole('button', { name: '前往创意工坊' });
    fireEvent.click(button);
    expect(mockInvoke).toHaveBeenCalledWith('open_url', {
      url: 'steam://url/CommunityFilePage/12345678',
    });
    
    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(false);
    });
  });
});
