import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { LinkConfirmModal } from './LinkConfirmModal';

describe('LinkConfirmModal', () => {
  test('renders nothing when open is false', () => {
    const { container } = render(
      <LinkConfirmModal
        open={false}
        url="https://example.com"
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />
    );
    expect(container.firstChild).toBeNull();
  });

  test('renders title, description, url and buttons when open is true', () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();

    render(
      <LinkConfirmModal
        open={true}
        url="https://example.com"
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    );

    expect(screen.getByText('即将访问外部网站')).toBeDefined();
    expect(screen.getByText('https://example.com')).toBeDefined();
    expect(screen.getByText('取消')).toBeDefined();
    expect(screen.getByText('继续访问')).toBeDefined();
  });

  test('calls onCancel when cancel is clicked', () => {
    const onCancel = vi.fn();

    render(
      <LinkConfirmModal
        open={true}
        url="https://example.com"
        onCancel={onCancel}
        onConfirm={vi.fn()}
      />
    );

    fireEvent.click(screen.getByText('取消'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  test('calls onConfirm with url when continue is clicked', () => {
    const onConfirm = vi.fn();

    render(
      <LinkConfirmModal
        open={true}
        url="https://example.com"
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('继续访问'));
    expect(onConfirm).toHaveBeenCalledWith('https://example.com');
  });
});
