import { describe, test, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { TransHTML, TransParagraphs } from './TransHTML';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string) => {
      if (key === 'array-key') return ['Line 1 <b>bold</b>', 'Line 2'];
      if (key === 'string-key') return 'Hello <i>world</i>';
      return key;
    },
  }),
}));

describe('TransHTML & TransParagraphs', () => {
  describe('TransHTML', () => {
    test('renders string translations with HTML structure', () => {
      const { container } = render(
        <TransHTML i18nKey="string-key" className="custom-class" />
      );

      const span = container.querySelector('.custom-class');
      expect(span).toBeDefined();
      expect(span?.innerHTML).toBe('Hello <i>world</i>');
    });

    test('renders array translations as separate divs with HTML structure', () => {
      const { container } = render(
        <TransHTML i18nKey="array-key" />
      );

      const wrapper = container.firstElementChild;
      expect(wrapper?.children.length).toBe(2);
      expect(wrapper?.children[0].innerHTML).toBe('Line 1 <b>bold</b>');
      expect(wrapper?.children[1].innerHTML).toBe('Line 2');
    });
  });

  describe('TransParagraphs', () => {
    test('renders string translations directly', () => {
      const { container } = render(
        <TransParagraphs i18nKey="string-key" />
      );

      expect(container.textContent).toBe('Hello <i>world</i>');
    });

    test('renders array translations joined by br tags', () => {
      const { container } = render(
        <TransParagraphs i18nKey="array-key" />
      );

      expect(container.textContent).toBe('Line 1 <b>bold</b>Line 2');
      const br = container.querySelector('br');
      expect(br).not.toBeNull();
    });
  });
});
