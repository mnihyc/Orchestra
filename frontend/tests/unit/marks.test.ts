/** Every mark the page makes has the one prefix, and a page that cannot make marks works as it did. */
import { afterEach, expect, it, vi } from 'vitest';

import { mark, whenContentShown } from '@/lib/marks';
import { compareRuns, pageOutcome, RequestLedger } from '../e2e/page-measurement';

afterEach(() => { vi.unstubAllGlobals(); });

it('names every moment under the one prefix, with what it says of where it matters', () => {
  const made = vi.fn();
  vi.stubGlobal('performance', { now: () => 0, mark: made });
  mark('start/script');
  mark('switch/click', { tab: 'tab-1' });
  expect(made.mock.calls).toEqual([['orch/start/script', undefined], ['orch/switch/click', { detail: { tab: 'tab-1' } }]]);
});

it('makes nothing, and does not fail, where the browser has no marks', () => {
  vi.stubGlobal('performance', {});
  expect(() => { mark('start/script'); }).not.toThrow();
});

it('releases background work only for the reader whose actual page content was shown', async () => {
  mark('start/script');
  const away = new AbortController();
  const done = vi.fn();
  const waiting = whenContentShown('reader', 2, away.signal).then(done);
  mark('page/content', { user: 'someone-else', reader: '2' });
  mark('page/content', { user: 'reader', reader: '1' });
  await Promise.resolve();
  expect(done).not.toHaveBeenCalled();
  mark('page/content', { user: 'reader', reader: '2' });
  await waiting;
  expect(done).toHaveBeenCalledWith(true);
});

it('lets go of a background wait when its reader leaves before content arrives', async () => {
  mark('start/script');
  const away = new AbortController();
  const waiting = whenContentShown('departed-reader', 1, away.signal);
  away.abort();
  expect(await waiting).toBe(false);
});

it('counts a call that began before content even while its answer is still held', () => {
  const ledger = new RequestLedger<object>();
  const held = {}, later = {};
  ledger.begin(held, '/api/v1/nodes', 10, 'fetch');
  ledger.begin(later, '/api/v1/me/bootstrap', 40, 'fetch');
  expect(ledger.before(30)).toMatchObject([{ path: '/api/v1/nodes', decoded: null, wire: null, end: null }]);
  ledger.finish(held, { decoded: 120, wire: 80, end: 90, failed: false });
  expect(ledger.before(30)).toMatchObject([{ path: '/api/v1/nodes', decoded: 120, wire: 80, end: 90 }]);
});

it('measures a tab return from its actual click without counting the previous pane\'s earlier work', () => {
  const ledger = new RequestLedger<string>();
  ledger.begin('previous', '/api/v1/old-pane', 10, 'fetch');
  ledger.begin('return', '/api/v1/returned-pane', 25, 'fetch');
  expect(ledger.before(30, 20).map((request) => request.path)).toEqual(['/api/v1/returned-pane']);
});

it('refuses another route, hidden selectors and native error content as successful page content', () => {
  const wanted = 'https://orch.example/thread/one?tab=work';
  expect(pageOutcome(wanted, 'https://orch.example/login', null, false)).toBe('redirect');
  expect(pageOutcome(wanted, wanted, 'error', true)).toBe('error');
  expect(pageOutcome(wanted, wanted, 'ready', false)).toBe('waiting');
  expect(pageOutcome(wanted, wanted, 'ready', true)).toBe('ready');
  expect(pageOutcome(wanted, 'https://orch.example/thread/one?tab=other', 'ready', true)).toBe('redirect');
});

it('compares retained tab returns with the same accepted scenario and reports added reads', () => {
  expect(compareRuns({ scenario: 'switch', calls: 1, bytes: 100, content: 25 }, { scenario: 'switch', calls: 0, bytes: 0, content: 20 }))
    .toEqual({ calls: 1, bytes: 100, content: 5 });
  expect(() => compareRuns({ scenario: 'repeat', calls: 1, bytes: 100, content: 25 }, { scenario: 'switch', calls: 0, bytes: 0, content: 20 })).toThrow('scenario');
});
