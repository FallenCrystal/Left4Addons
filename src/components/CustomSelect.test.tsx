import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { CustomSelect } from './CustomSelect';

describe('CustomSelect', () => {
  const options = [
    { value: 'opt1', label: 'Option 1' },
    { value: 'opt2', label: 'Option 2' },
  ];

  test('renders current value label', () => {
    render(
      <CustomSelect
        options={options}
        value="opt1"
        onChange={vi.fn()}
      />
    );
    expect(screen.getByText('Option 1')).toBeDefined();
    expect(screen.queryByText('Option 2')).toBeNull();
  });

  test('opens dropdown on click and triggers onChange', () => {
    const onChange = vi.fn();
    render(
      <CustomSelect
        options={options}
        value="opt1"
        onChange={onChange}
      />
    );

    // Click trigger to open dropdown
    fireEvent.click(screen.getByText('Option 1'));

    // Option 2 should now be visible
    const opt2Element = screen.getByText('Option 2');
    expect(opt2Element).toBeDefined();

    // Click Option 2
    fireEvent.click(opt2Element);
    expect(onChange).toHaveBeenCalledWith('opt2');

    // Dropdown should be closed now
    expect(screen.queryByText('Option 2')).toBeNull();
  });
});
