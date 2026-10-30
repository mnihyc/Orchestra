/** Authored links cannot masquerade as local paths or break the reader. */
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Body } from '@/features/chat/Body';
import { linkAllowed } from '@/features/people/links';
import { isSafeHref, lexMarkdown, renderBlocks } from '@/lib/markdown';

describe('authored content links', () => {
  it('keeps disguised external addresses inert in Markdown while preserving valid links', () => {
    const source = String.raw`[bad](</\\attacker.example/path>) [web](https://example.org/a) [local](/help)`;
    const page = render(<div>{renderBlocks(lexMarkdown(source))}</div>).container;
    expect(page.textContent).toContain('bad');
    expect(page.querySelectorAll('a')).toHaveLength(2);
    expect([...page.querySelectorAll('a')].map((link) => link.getAttribute('href'))).toEqual(['https://example.org/a', '/help']);
  });

  it('does not accept protocol-relative addresses disguised by slashes or control characters', () => {
    for (const address of ['//attacker.example/a', '/\\attacker.example/a', '/\t/attacker.example/a', '/\n/attacker.example/a']) {
      expect(isSafeHref(address), JSON.stringify(address)).toBe(false);
      expect(linkAllowed(address), JSON.stringify(address)).toBe(false);
    }
    for (const address of ['/help', '#part', 'https://example.org/a', 'mailto:a@example.org']) {
      expect(isSafeHref(address), address).toBe(true);
      expect(linkAllowed(address), address).toBe(true);
    }
  });

  it('does not emit disguised protocol-relative links in chat', () => {
    const text = String.raw`[slashes](//attacker.example/a) [backslash](/\attacker.example/a) [web](https://example.org/a) [local](/help)`;
    const page = render(<Body text={text} />).container;
    expect(page.textContent).toContain('slashes');
    expect(page.textContent).toContain('backslash');
    expect([...page.querySelectorAll('a')].map((link) => link.getAttribute('href'))).toEqual(['https://example.org/a', '/help']);
  });

  it('keeps a malformed escaped place address readable in chat', () => {
    const page = render(<Body text="Before [open](/thread/%zz) after" />).container;
    expect(page.textContent).toContain('Before open after');
  });
});
