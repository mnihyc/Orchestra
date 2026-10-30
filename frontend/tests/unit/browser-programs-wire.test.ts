/** Smaller live rows never masquerade as a complete browser/display answer or erase its capability facts. */
import { QueryClient } from '@tanstack/react-query';
import { expect, it } from 'vitest';

import { applyQueryFrame } from '@/realtime/queryFeeds';
import type { FeedFrame } from '@/realtime/socket';
import { browserSnapshot, displaySnapshot, runningBrowser } from '../component/browser-discovery-fixtures';

function frame(kind: 'browsers' | 'displays', data: Record<string, unknown>): FeedFrame {
  return { op: 'sync', feed: `env:environment:${kind}`, id: 1, prev: 0, seq: 1, data };
}

for (const kind of ['browsers', 'displays'] as const) {
  it(`does not seed ${kind} from a cold row-only or missing-program snapshot`, () => {
    const cache = new QueryClient(), key = [`environment-${kind}`, 'environment'];
    const full = kind === 'browsers' ? browserSnapshot() : displaySnapshot();
    const rows = kind === 'browsers' ? { browsers: [], profiles: [] } : { displays: [] };
    applyQueryFrame(cache, key, frame(kind, rows));
    expect(cache.getQueryData(key)).toBeUndefined();
    applyQueryFrame(cache, key, frame(kind, { ...full, support: {} }));
    expect(cache.getQueryData(key)).toBeUndefined();
    applyQueryFrame(cache, key, frame(kind, { ...full }));
    expect(cache.getQueryData(key)).toEqual(full);
    cache.clear();
  });

  it(`keeps the full ${kind} projection when a late observation omits capability fields`, () => {
    const cache = new QueryClient(), key = [`environment-${kind}`, 'environment'];
    const full = kind === 'browsers' ? browserSnapshot() : displaySnapshot();
    cache.setQueryData(key, full);
    applyQueryFrame(cache, key, frame(kind, { support: {} }));
    expect(cache.getQueryData(key)).toBe(full);
    applyQueryFrame(cache, key, frame(kind, { off: 'environment_offline' }));
    expect(cache.getQueryData(key)).toEqual({ ...full, off: 'environment_offline' });
    cache.clear();
  });
}

it('merges the browser feed rows around the HTTP support, presets, profile directory and limit', () => {
  const cache = new QueryClient(), key = ['environment-browsers', 'environment'];
  const initial = runningBrowser(), browser = { ...initial, tabs: 3 };
  const full = { ...browserSnapshot(), browsers: [initial], limit: { open: 1, max: 4 } };
  cache.setQueryData(key, full);
  const profiles = [{ name: 'work', size_bytes: null, used_at: null, browser: browser.id }];
  applyQueryFrame(cache, key, frame('browsers', { browsers: [browser], profiles }));
  expect(cache.getQueryData(key)).toEqual({ ...full, browsers: [browser], profiles });
  cache.clear();
});
