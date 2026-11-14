/** A tab opened for an address starts the read of its page at that moment, and the screen that comes later takes it over. */
import { afterEach, beforeEach, expect, it } from 'vitest';

import { openContent } from '@/features/workbench/contentNavigation';
import { installPageReadStarters, pageTookReads } from '@/features/workbench/pageReads';
import { useWorkbench } from '@/features/workbench/store';

const READER = '01950000-0000-7000-8000-000000000001';
const THREAD = '01950000-0000-7000-8000-0000000000bb';

beforeEach(() => { useWorkbench.getState().leave(); useWorkbench.getState().enter(READER, 'thread'); });
afterEach(() => { installPageReadStarters({}); useWorkbench.getState().leave(); });

it('starts the read of a thread when its tab is opened, once for each tab, and lets go of it when the screen has taken it over', () => {
  const started: [string, string][] = [];
  const released: string[] = [];
  installPageReadStarters({ thread: (thread, tab) => { started.push([thread, tab]); return () => { released.push(tab); }; } });
  const first = openContent(`/thread/${THREAD}`);
  if (first === null) throw new Error('The address opens no tab');
  expect(started).toEqual([[THREAD, first]]);
  // The same address again brings the open tab forward: nothing starts twice.
  expect(openContent(`/thread/${THREAD}`)).toBe(first);
  expect(started).toHaveLength(1);
  // A tab opened beside reads for itself.
  const beside = openContent(`/thread/${THREAD}`, 'new');
  if (beside === null) throw new Error('The address opens no second tab');
  expect(started).toEqual([[THREAD, first], [THREAD, beside]]);
  // The screen of the first tab follows what was started; the tab's own hold ends once.
  pageTookReads(first);
  pageTookReads(first);
  expect(released).toEqual([first]);
  // A page that reads no thread starts nothing.
  openContent('/inbox');
  expect(started).toHaveLength(2);
});
