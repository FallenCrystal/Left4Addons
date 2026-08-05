import React from 'react';
import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import { renderTextWithLinks } from './linkHelper';

describe('renderTextWithLinks', () => {
  test('renders plain text without links as-is', () => {
    const text = 'Hello world, this is a plain description.';
    render(<div>{renderTextWithLinks(text)}</div>);
    expect(screen.getByText(text)).toBeTruthy();
  });

  test('converts standard urls into external links', () => {
    const text = 'Check out https://google.com for more info.';
    const onOpenLink = vi.fn();
    render(<div>{renderTextWithLinks(text, undefined, onOpenLink)}</div>);

    const link = screen.getByText('https://google.com');
    expect(link).toBeTruthy();
    
    fireEvent.click(link);
    expect(onOpenLink).toHaveBeenCalledWith('https://google.com');
  });

  test('converts steam workshop links and triggers direct navigation', () => {
    const text = 'Check out the collection at https://steamcommunity.com/sharedfiles/filedetails/?id=284729104!';
    const onDirectNavigate = vi.fn();
    const onOpenLink = vi.fn();
    render(<div>{renderTextWithLinks(text, onDirectNavigate, onOpenLink)}</div>);

    const link = screen.getByText('https://steamcommunity.com/sharedfiles/filedetails/?id=284729104');
    expect(link).toBeTruthy();

    fireEvent.click(link);
    expect(onDirectNavigate).toHaveBeenCalledWith('284729104');
    expect(onOpenLink).not.toHaveBeenCalled();
  });
});
