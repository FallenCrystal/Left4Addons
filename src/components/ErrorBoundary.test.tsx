import { describe, test, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ErrorBoundary } from './ErrorBoundary';
import React from 'react';

const ThrowError = () => {
  throw new Error('Test Render Error');
};

describe('ErrorBoundary', () => {
  const reloadMock = vi.fn();

  beforeAll(() => {
    // Suppress console.error output from expected React errors in test logs
    vi.spyOn(console, 'error').mockImplementation(() => {});

    Object.defineProperty(window, 'location', {
      configurable: true,
      value: { reload: reloadMock },
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  test('renders children if no error is thrown', () => {
    render(
      <ErrorBoundary>
        <div>Safe Component</div>
      </ErrorBoundary>
    );

    expect(screen.getByText('Safe Component')).toBeDefined();
  });

  test('renders fallback UI and details when a child component throws', () => {
    render(
      <ErrorBoundary>
        <ThrowError />
      </ErrorBoundary>
    );

    expect(screen.getByText('应用遇到意外错误 (Frontend Crash)')).toBeDefined();
    expect(screen.getByText('Error: Error: Test Render Error')).toBeDefined();
    expect(screen.getByText('尝试恢复界面')).toBeDefined();
    expect(screen.getByText('重新加载应用')).toBeDefined();
  });

  test('resets error state when clicking recover button', () => {
    const { rerender } = render(
      <ErrorBoundary>
        <ThrowError />
      </ErrorBoundary>
    );

    expect(screen.getByText('应用遇到意外错误 (Frontend Crash)')).toBeDefined();

    // Rerender with a safe component first (but still in error state)
    rerender(
      <ErrorBoundary>
        <div>Recovered Component</div>
      </ErrorBoundary>
    );

    // Click recover/reset button
    fireEvent.click(screen.getByText('尝试恢复界面'));

    expect(screen.getByText('Recovered Component')).toBeDefined();
  });

  test('reloads page when clicking reload button', () => {
    render(
      <ErrorBoundary>
        <ThrowError />
      </ErrorBoundary>
    );

    fireEvent.click(screen.getByText('重新加载应用'));
    expect(reloadMock).toHaveBeenCalledTimes(1);
  });
});
