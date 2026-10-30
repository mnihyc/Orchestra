// @vitest-environment node
/** The actual theme cannot make interactive boundaries or selected options fade into their surfaces. */
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { contrast } from '../accessibility/contrast';

const css = readFileSync(new URL('../../src/styles/tokens.css', import.meta.url), 'utf8');
const surfaces = ['bg', 'bg-subtle', 'bg-muted', 'bg-inset', 'surface', 'surface-raised', 'accent-subtle', 'success-subtle', 'warning-subtle', 'danger-subtle', 'info-subtle'];

function palette(theme: 'light' | 'dark'): (name: string) => string {
  const block = (theme === 'light' ? /:root\s*\{([^}]+)\}/u : /\[data-theme='dark'\]\s*\{([^}]+)\}/u).exec(css)?.[1];
  if (block === undefined) throw new Error(`Missing ${theme} theme`);
  const values = new Map([...block.matchAll(/--([\w-]+):\s*([^;]+);/gu)].map((match) => [match[1] ?? '', (match[2] ?? '').trim()]));
  return (name) => {
    const seen = new Set<string>();
    let next = name;
    while (!seen.has(next)) {
      seen.add(next);
      const value = values.get(next);
      if (value === undefined) throw new Error(`Missing ${theme} colour ${next}`);
      const alias = /^var\(--([\w-]+)\)$/u.exec(value)?.[1];
      if (alias === undefined) return value;
      next = alias;
    }
    throw new Error(`Cyclic ${theme} colour ${name}`);
  };
}

describe.each(['light', 'dark'] as const)('selector contrast in %s', (theme) => {
  const colour = palette(theme);
  it.each(surfaces)('keeps enabled resource action words above 4.5:1 on %s', (surface) => {
    expect(contrast(colour('text'), colour(surface))).toBeGreaterThanOrEqual(4.5);
  });
  it.each(surfaces)('keeps its boundary and selected surface above 3:1 on %s', (surface) => {
    expect(contrast(colour('control-border'), colour(surface))).toBeGreaterThanOrEqual(3);
    expect(contrast(colour('control-selected'), colour(surface))).toBeGreaterThanOrEqual(3);
  });
  it('keeps selected words readable and the switch knob distinct on either track', () => {
    expect(contrast(colour('control-on-selected'), colour('control-selected'))).toBeGreaterThanOrEqual(4.5);
    expect(contrast(colour('text-on-accent'), colour('control-border'))).toBeGreaterThanOrEqual(3);
  });
  it('keeps the words of a chosen segment readable on its raised ground', () => {
    expect(contrast(colour('text'), colour('segment-selected'))).toBeGreaterThanOrEqual(4.5);
  });
});

it('tells the chosen segment from its track in the dark theme, where a shadow does not show', () => {
  const colour = palette('dark');
  expect(contrast(colour('segment-selected'), colour('bg-muted'))).toBeGreaterThanOrEqual(1.4);
});
