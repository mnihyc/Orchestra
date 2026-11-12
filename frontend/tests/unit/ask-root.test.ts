/** The actual root hook retains an explicit private choice only for its current page. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { useAskRoot } from '@/features/ask/useAskRoot';

interface Props { pageRoot: string | null; seed: Parameters<typeof useAskRoot>[1] }
const draw = (props: Props) => renderHook(({ pageRoot, seed }: Props) => useAskRoot(pageRoot, seed), { initialProps: props });
afterEach(cleanup);

it('keeps the source root after consumption, follows a new page, and never resurrects the old override', () => {
  const view = draw({ pageRoot: 'root-a', seed: { id: 'private-selection', root: 'root-b' } });
  expect(view.result.current[0]).toBe('root-b');
  view.rerender({ pageRoot: 'root-a', seed: null });
  expect(view.result.current[0]).toBe('root-b');
  view.rerender({ pageRoot: 'root-c', seed: null });
  expect(view.result.current[0]).toBe('root-c');
  view.rerender({ pageRoot: 'root-a', seed: null });
  expect(view.result.current[0]).toBe('root-a');
});

it('retains the private root while the page context resolves before consuming the seed', () => {
  const seed = { id: 'pending-source', root: 'root-b' };
  const view = draw({ pageRoot: null, seed });
  expect(view.result.current[0]).toBe('root-b');
  view.rerender({ pageRoot: 'root-a', seed });
  expect(view.result.current[0]).toBe('root-b');
  view.rerender({ pageRoot: 'root-a', seed: null });
  expect(view.result.current[0]).toBe('root-b');
});

it('keeps an explicit history choice through public Help context, then follows navigation normally', () => {
  const view = draw({ pageRoot: 'root-a', seed: null });
  act(() => view.result.current[1]('root-b'));
  expect(view.result.current[0]).toBe('root-b');
  view.rerender({ pageRoot: 'root-a', seed: { id: 'public-help', root: null } });
  expect(view.result.current[0]).toBe('root-b');
  view.rerender({ pageRoot: 'root-a', seed: null });
  expect(view.result.current[0]).toBe('root-b');
  view.rerender({ pageRoot: 'root-c', seed: null });
  expect(view.result.current[0]).toBe('root-c');
});

it('lets a newer explicit source supersede an older pin without transferring it to a new panel', () => {
  const view = draw({ pageRoot: 'root-a', seed: { id: 'first', root: 'root-b' } });
  view.rerender({ pageRoot: 'root-a', seed: { id: 'second', root: 'root-c' } });
  expect(view.result.current[0]).toBe('root-c');
  view.rerender({ pageRoot: 'root-a', seed: null });
  expect(view.result.current[0]).toBe('root-c');
  view.unmount();
  const next = draw({ pageRoot: 'root-a', seed: null });
  expect(next.result.current[0]).toBe('root-a');
});
