/** Authored links cannot masquerade as local paths or break the reader. */
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Body } from '@/features/chat/Body';
import { readHtml } from '@/features/people/html';
import { linkAllowed } from '@/features/people/links';
import { isSafeHref, lexMarkdown, renderBlocks } from '@/lib/markdown';

describe('authored content links', () => {
  it('opens a disguised external Markdown address as an isolated web link', () => {
    const source = String.raw`[bad](</\\attacker.example/path>) [web](https://example.org/a) [local](/help)`;
    const page = render(<div>{renderBlocks(lexMarkdown(source))}</div>).container;
    expect(page.textContent).toContain('bad');
    const links = [...page.querySelectorAll('a')];
    expect(links.map((link) => link.getAttribute('href'))).toEqual([new URL('/\\attacker.example/path', location.href).href, 'https://example.org/a', '/help']);
    expect(links[0]?.getAttribute('target')).toBe('_blank');
    expect(links[0]?.getAttribute('rel')).toBe('noopener noreferrer');
  });

  it('does not accept protocol-relative addresses disguised by slashes or control characters', () => {
    for (const address of ['//attacker.example/a', '/\\attacker.example/a', '/\t/attacker.example/a', '/\n/attacker.example/a', '/\u0085/attacker.example/a']) {
      expect(isSafeHref(address), JSON.stringify(address)).toBe(false);
      expect(linkAllowed(address), JSON.stringify(address)).toBe(false);
    }
    for (const address of ['/help', '#part', 'https://example.org/a', 'mailto:a@example.org']) {
      expect(isSafeHref(address), address).toBe(true);
      expect(linkAllowed(address), address).toBe(true);
    }
  });

  it('opens protocol-relative chat links as isolated external web links', () => {
    const text = String.raw`[slashes](//attacker.example/a) [backslash](/\attacker.example/a) [web](https://example.org/a) [local](/help)`;
    const page = render(<Body text={text} />).container;
    expect(page.textContent).toContain('slashes');
    expect(page.textContent).toContain('backslash');
    const links = [...page.querySelectorAll('a')];
    expect(links.map((link) => link.getAttribute('href'))).toEqual([new URL('//attacker.example/a', location.href).href, new URL('/\\attacker.example/a', location.href).href, 'https://example.org/a', '/help']);
    expect(links.slice(0, 2).map((link) => [link.getAttribute('target'), link.getAttribute('rel')])).toEqual([['_blank', 'noreferrer noopener'], ['_blank', 'noreferrer noopener']]);
  });

  it('keeps a malformed escaped place address readable in chat', () => {
    const page = render(<Body text="Before [open](/thread/%zz) after" />).container;
    expect(page.textContent).toContain('Before open after');
  });

  it('leaves disguised unsafe links as text while converting HTML', () => {
    const source = String.raw`<p><a href="/\attacker.example/a">backslash</a> <a href="/&#x09;/attacker.example/a">control</a> <a href="relative.md">relative</a> <a href="https://example.org/a">web</a></p>`;
    const result = readHtml(source);
    const runs = result.pieces[0]?.content as { text: string; href?: string }[];
    expect(runs.find((run) => run.text === 'backslash')?.href).toBeUndefined();
    expect(runs.find((run) => run.text === 'control')?.href).toBeUndefined();
    expect(runs.find((run) => run.text === 'relative')?.href).toBe('relative.md');
    expect(runs.find((run) => run.text === 'web')?.href).toBe('https://example.org/a');
    expect(result.warnings).toContain('Unsafe links were left as text.');
  });
});
