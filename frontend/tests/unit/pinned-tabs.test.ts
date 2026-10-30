/** Pins preserve their order and content through moves, bulk close and stored layout upgrades. */
import { beforeEach, expect, it } from 'vitest';
import { addMainTab, findTab, LAYOUT_VERSION, mainGroups, makeGroup, move, normalize, parse, serialize, setPinned, tabsForClose, type Layout, type Tab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';

const tab = (id: string, pinned = false): Tab => ({ id, view: 'page', of: { href: `/${id}` }, ...(pinned ? { pinned: true } : {}) });
function layout(): Layout {
  return { version: LAYOUT_VERSION, main: makeGroup([tab('one'), tab('two'), tab('everything')]), maximized: null,
    left: { open: false, size: 280, active: null, containers: [] }, right: { open: false, size: 400, active: null, containers: [] },
    bottom: { open: false, size: 240, group: makeGroup() } };
}
const ids = (given: Layout) => mainGroups(given.main)[0]!.tabs.map((tab) => tab.id);
beforeEach(() => { useWorkbench.getState().leave(); });

it('pins first in stable order and lets either drag sensor cross the pin boundary', () => {
  let given = setPinned(setPinned(layout(), 'two', true), 'everything', true);
  const group = mainGroups(given.main)[0]!.id;
  expect(ids(given)).toEqual(['two', 'everything', 'one']);
  given = move(given, 'everything', { to: 'group', group, index: 0 });
  expect(ids(given)).toEqual(['everything', 'two', 'one']);
  expect(findTab(given, 'everything')!.tab.pinned).toBe(true);
  given = move(given, 'one', { to: 'group', group, index: 0 });
  expect(ids(given)).toEqual(['one', 'everything', 'two']);
  expect(findTab(given, 'one')!.tab.pinned).toBe(true);
  given = setPinned(given, 'two', false);
  given = move(given, 'everything', { to: 'group', group });
  expect(ids(given)).toEqual(['one', 'two', 'everything']);
  expect(findTab(given, 'everything')!.tab.pinned).not.toBe(true);
});

it('keeps pins for close others and close all, with Close in its own menu as the explicit exception', () => {
  const given = setPinned(layout(), 'everything', true);
  const group = mainGroups(given.main)[0]!.id;
  expect(tabsForClose(given, { kind: 'main' }).map((tab) => tab.id)).toEqual(['one', 'two']);
  expect(tabsForClose(given, { kind: 'group', group }).map((tab) => tab.id)).toEqual(['one', 'two']);
  expect(tabsForClose(given, { kind: 'group-others', tab: 'one' }).map((tab) => tab.id)).toEqual(['two']);
  expect(tabsForClose(given, { kind: 'tab', tab: 'everything' })).toEqual([]);
  expect(tabsForClose(given, { kind: 'tab', tab: 'everything', explicit: true }).map((tab) => tab.id)).toEqual(['everything']);
});

it('preserves pins across groups, normalized insertion, Reset, arrangements and reload', () => {
  const initial = setPinned(layout(), 'everything', true);
  let given = move(initial, 'everything', { to: 'split', group: mainGroups(initial.main)[0]!.id, side: 'right' });
  expect(findTab(given, 'everything')!.tab.pinned).toBe(true);
  const group = mainGroups(given.main)[0]!.id;
  given = move(given, 'everything', { to: 'group', group });
  expect(ids(given)[0]).toBe('everything');
  given = addMainTab(given, tab('three'), group);
  expect(ids(normalize(given))).toEqual(['everything', 'one', 'two', 'three']);
  expect(parse(serialize(given))).toEqual(given);
  useWorkbench.setState({ layout: given, screen: 'page' });
  for (const choice of ['reset', 'conversation', 'code', 'review']) {
    if (choice === 'reset') useWorkbench.getState().reset(); else useWorkbench.getState().choose(choice);
    expect(findTab(useWorkbench.getState().layout, 'everything')?.tab.pinned).toBe(true);
  }
  for (const version of [2, 3]) expect(parse(serialize({ ...given, version }))?.version).toBe(LAYOUT_VERSION);
});
