import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import { RequiredItems } from './WorkshopCommon';

describe('WorkshopCommon', () => {
  test('dependency link click does not bubble to parent click handlers', () => {
    const onParentClick = vi.fn();
    const onItemNavigate = vi.fn();

    render(
      <div onClick={onParentClick}>
        <RequiredItems
          requiredItems={[{ title: 'Dependency Addon', workshopId: '12345' }]}
          addons={{}}
          knownUninstalledAddons={{}}
          onItemNavigate={onItemNavigate}
        />
      </div>,
    );

    fireEvent.click(screen.getByText('Dependency Addon'));

    expect(onItemNavigate).toHaveBeenCalledWith('12345');
    expect(onParentClick).not.toHaveBeenCalled();
  });
});
