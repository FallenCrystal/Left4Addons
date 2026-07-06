import { describe, test, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { EditGroupModal } from './EditGroupModal';
import { Addon } from '../types/addon';

describe('EditGroupModal', () => {
  const confirmMock = vi.fn();

  beforeAll(() => {
    Object.defineProperty(window, 'confirm', {
      configurable: true,
      value: confirmMock,
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  const mockAddons: Addon[] = [
    {
      vpkName: 'addon1.vpk',
      dirType: 'loading',
      isEnabled: true,
      fileSize: 100,
      filesCount: 1,
      addonInfo: {
        addonContent_Campaign: '1',
      },
    },
  ];

  test('renders nothing when open is false', () => {
    const { container } = render(
      <EditGroupModal
        open={false}
        groupId="g1"
        currentName="My Group"
        addonsInGroup={[]}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders fields and handles edit flow', () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();

    render(
      <EditGroupModal
        open={true}
        groupId="g1"
        currentName="My Group"
        currentTags={['tag1', 'tag2']}
        currentCollectionId="12345"
        addonsInGroup={mockAddons}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    );

    expect(screen.getByText('编辑分组')).toBeDefined();
    
    const nameInput = screen.getByDisplayValue('My Group') as HTMLInputElement;
    expect(nameInput).toBeDefined();

    const tagsInput = screen.getByPlaceholderText('例如: 战役, 武器模型, 皮肤') as HTMLInputElement;
    expect(tagsInput.value).toBe('tag1, tag2');

    const colInput = screen.getByPlaceholderText('例如: 3560901999') as HTMLInputElement;
    expect(colInput.value).toBe('12345');

    fireEvent.click(screen.getByText('取消'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  test('auto allocates tags correctly', () => {
    render(
      <EditGroupModal
        open={true}
        groupId="g1"
        currentName="My Group"
        currentTags={[]}
        addonsInGroup={mockAddons}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );

    fireEvent.click(screen.getByText('自动分配标签'));
    const tagsInput = screen.getByPlaceholderText('例如: 战役, 武器模型, 皮肤') as HTMLInputElement;
    // mockAddons has Campaign enabled
    expect(tagsInput.value).toBe('Campaign');
  });

  test('calls onConfirm with parameters on submit without collection change', () => {
    const onConfirm = vi.fn();
    render(
      <EditGroupModal
        open={true}
        groupId="g1"
        currentName="My Group"
        currentTags={['tag1']}
        currentCollectionId="12345"
        addonsInGroup={[]}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const nameInput = screen.getByDisplayValue('My Group') as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: 'New Name' } });

    const form = nameInput.closest('form');
    if (form) {
      fireEvent.submit(form);
      expect(onConfirm).toHaveBeenCalledWith('g1', 'New Name', ['tag1'], '12345');
    }
  });

  test('prompts confirmation when collection ID changes and submits only on confirm', () => {
    const onConfirm = vi.fn();
    confirmMock.mockReturnValue(false);

    render(
      <EditGroupModal
        open={true}
        groupId="g1"
        currentName="My Group"
        currentTags={[]}
        currentCollectionId=""
        addonsInGroup={[]}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const colInput = screen.getByPlaceholderText('例如: 3560901999') as HTMLInputElement;
    fireEvent.change(colInput, { target: { value: '999' } });

    // Warning should show up
    expect(screen.getByText('安全警告：')).toBeDefined();

    const form = colInput.closest('form');
    if (form) {
      fireEvent.submit(form);
      expect(confirmMock).toHaveBeenCalledTimes(1);
      expect(onConfirm).not.toHaveBeenCalled();

      // Rerender with confirmMock returning true
      confirmMock.mockReturnValue(true);
      fireEvent.submit(form);
      expect(confirmMock).toHaveBeenCalledTimes(2);
      expect(onConfirm).toHaveBeenCalledWith('g1', 'My Group', [], '999');
    }
  });
});
