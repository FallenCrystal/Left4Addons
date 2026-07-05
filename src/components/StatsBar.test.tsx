import { describe, test, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { StatsBar } from './StatsBar';

describe('StatsBar', () => {
  test('renders all stats correct formatted', () => {
    render(
      <StatsBar
        totalAddonsCount={42}
        activeCount={30}
        disabledCount={12}
        totalStorageSize={1024 * 1024 * 1024 * 2.5} // 2.5 GB
      />
    );

    // Assert total count
    expect(screen.getByText('42')).toBeDefined();
    expect(screen.getByText('总附件数')).toBeDefined();

    // Assert active count
    expect(screen.getByText('30')).toBeDefined();
    expect(screen.getByText('已启用')).toBeDefined();

    // Assert disabled count
    expect(screen.getByText('12')).toBeDefined();
    expect(screen.getByText('已禁用')).toBeDefined();

    // Assert size formatted
    expect(screen.getByText('2.5 GB')).toBeDefined();
    expect(screen.getByText('总磁盘空间')).toBeDefined();
  });
});
