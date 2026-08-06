import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { SettingsModal } from './SettingsModal';

const mockInvoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mockInvoke(cmd, args),
}));

describe('SettingsModal', () => {
  test('renders nothing when open is false', () => {
    const { container } = render(
      <SettingsModal
        open={false}
        initialLoadingDir="C:\\Games\\L4D2\\left4dead2\\addons"
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders form fields when open is true', () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();

    const initialLoadingDir = "C:\\Games\\L4D2\\left4dead2\\addons";
    render(
      <SettingsModal
        open={true}
        initialLoadingDir={initialLoadingDir}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    );

    expect(screen.getByText('设置附加组件加载路径')).toBeDefined();
    
    const input = screen.getByRole('textbox') as HTMLInputElement;
    expect(input.value).toBe(initialLoadingDir);

    expect(screen.getByText('取消')).toBeDefined();
    expect(screen.getByText('保存并重新扫描')).toBeDefined();
  });

  test('submitting form calls onConfirm with input path', () => {
    const onConfirm = vi.fn();

    render(
      <SettingsModal
        open={true}
        initialLoadingDir="C:\\Games\\L4D2\\left4dead2\\addons"
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    const input = screen.getByRole('textbox') as HTMLInputElement;
    fireEvent.change(input, { target: { value: '  D:\\Steam\\steamapps\\common\\Left 4 Dead 2\\left4dead2\\addons  ' } });

    const form = input.closest('form');
    if (form) {
      fireEvent.submit(form);
      expect(onConfirm).toHaveBeenCalledWith('D:\\Steam\\steamapps\\common\\Left 4 Dead 2\\left4dead2\\addons');
    }
  });

  test('clicking cancel calls onCancel', () => {
    const onCancel = vi.fn();

    render(
      <SettingsModal
        open={true}
        initialLoadingDir="C:\\Games\\L4D2\\left4dead2\\addons"
        onCancel={onCancel}
        onConfirm={vi.fn()}
      />
    );

    fireEvent.click(screen.getByText('取消'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  test('handles auto detect path in SettingsModal', async () => {
    mockInvoke.mockResolvedValueOnce('D:\\Steam\\steamapps\\common\\Left 4 Dead 2\\left4dead2\\addons');

    render(
      <SettingsModal
        open={true}
        initialLoadingDir=""
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );

    const autoDetectBtn = screen.getByText('自动查找');
    fireEvent.click(autoDetectBtn);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('auto_detect_addons_path', undefined);
      expect((screen.getByDisplayValue('D:\\Steam\\steamapps\\common\\Left 4 Dead 2\\left4dead2\\addons') as HTMLInputElement).value).toBe('D:\\Steam\\steamapps\\common\\Left 4 Dead 2\\left4dead2\\addons');
      expect(screen.getByText('已自动找到 Left 4 Dead 2 路径！')).toBeDefined();
    });
  });
});

