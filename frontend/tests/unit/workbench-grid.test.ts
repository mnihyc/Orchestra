/** Deterministic grids rearrange presentation while preserving resource and tab identities. */
import { expect, it } from 'vitest';
import { addMainTab, applyGrid, close, closeTabs, findTab, LAYOUT_VERSION, mainGroups, makeGroup, move, normalize,
  parse, removeEmptyMainGroup, serialize, spatialMainGroups, tabsForClose, type Layout, type Tab } from '@/features/workbench/layout';

const tab = (id: string): Tab => ({ id, view: 'screen', of: { kind: 'thread', id } });
function layout(tabs: Tab[]): Layout {
  return { version: LAYOUT_VERSION,
    left: { open: true, size: 280, active: 'navigation', containers: [{ id: 'left-group', name: 'navigation', active: 'nav',
      sections: [{ tab: { id: 'nav', view: 'navigation' }, open: true, hidden: false, size: 1 }] }] },
    right: { open: true, size: 400, active: 'code', containers: [{ id: 'right-group', name: 'code', active: 'files-a',
      sections: [{ tab: { id: 'files-a', view: 'files', of: { thread: 'a' } }, open: true, hidden: false, size: 1 }] }] },
    bottom: { open: true, size: 240, group: { kind: 'group', id: 'bottom-group', active: 'terminal', tabs: [{ id: 'terminal', view: 'terminal' }] } },
    main: { kind: 'group', id: 'original-group', active: tabs[0]?.id ?? null, tabs }, maximized: null };
}

it('reads nested columns spatially instead of exhausting the entire left column first', () => {
  const a = makeGroup([tab('a')]), b = makeGroup([tab('b')]), c = makeGroup([tab('c')]), d = makeGroup([tab('d')]);
  const main: Layout['main'] = { kind: 'split', id: 'outer', direction: 'row', sizes: [40, 60], children: [
    { kind: 'split', id: 'left-column', direction: 'column', sizes: [50, 50], children: [a, c] },
    { kind: 'split', id: 'right-column', direction: 'column', sizes: [50, 50], children: [b, d] },
  ] };
  expect(mainGroups(main).flatMap(group => group.tabs.map(one => one.id))).toEqual(['a', 'c', 'b', 'd']);
  expect(spatialMainGroups(main).flatMap(group => group.tabs.map(one => one.id))).toEqual(['a', 'b', 'c', 'd']);
  const next = applyGrid({ ...layout([]), main }, '2x4', 'd');
  expect(spatialMainGroups(next.main).slice(0, 4).flatMap(group => group.tabs.map(one => one.id))).toEqual(['a', 'b', 'c', 'd']);
});

it.each(['2x4', '3x3'] as const)('%s retains all identities, overflow focus, equal starting splits and the surrounding frame', (preset) => {
  const tabs = Array.from({ length: 12 }, (_, index) => tab(`t${index}`)), before = layout(tabs);
  const original = serialize(before), next = applyGrid(before, preset, 't11');
  const groups = spatialMainGroups(next.main), count = preset === '2x4' ? 8 : 9;
  expect(groups).toHaveLength(count); expect(groups.every(group => group.retainEmpty === true)).toBe(true);
  expect(groups.slice(0, -1).map(group => group.tabs.length)).toEqual(Array.from({ length: count - 1 }, () => 1));
  expect(groups.at(-1)?.tabs.map(one => one.id)).toEqual(tabs.slice(count - 1).map(one => one.id));
  expect(groups.at(-1)?.active).toBe('t11');
  expect(groups.flatMap(group => group.tabs)).toEqual(tabs);
  for (const one of tabs) expect(findTab(next, one.id)?.tab).toBe(one);
  expect(next.left).toEqual(before.left); expect(next.right).toEqual(before.right); expect(next.bottom).toEqual(before.bottom);
  expect(serialize(before)).toBe(original);
  if (next.main.kind !== 'split') throw new Error('The preset must use the existing split tree');
  expect(next.main.direction).toBe('column');
  expect(next.main.sizes).toEqual(preset === '2x4' ? [50, 50] : [100 / 3, 100 / 3, 100 / 3]);
  expect(parse(serialize(next))).toEqual(next);
});

it('retains requested empty cells through close, move, normalization, reload and a second preset', () => {
  const before = applyGrid(layout([tab('a'), tab('b')]), '3x3', 'b');
  const closed = close(before, 'a'); expect(mainGroups(closed.main)).toHaveLength(9);
  const source = findTab(closed, 'b')?.group.id, destination = spatialMainGroups(closed.main)[8]?.id;
  if (source === undefined || destination === undefined) throw new Error('Both grid cells must exist');
  const moved = move(closed, 'b', { to: 'group', group: destination });
  expect(mainGroups(moved.main)).toHaveLength(9);
  expect(mainGroups(moved.main).find(group => group.id === source)?.tabs).toEqual([]);
  expect(mainGroups(normalize(moved).main)).toHaveLength(9);
  expect(parse(serialize(moved))).toEqual(moved);
  const smaller = applyGrid(moved, '2x4', 'b');
  expect(spatialMainGroups(smaller.main)).toHaveLength(8);
  expect(findTab(smaller, 'b')?.tab).toBe(findTab(before, 'b')?.tab);
});

it('removes only an explicitly addressed empty grid group while manual empty splits still collapse', () => {
  const grid = applyGrid(layout([tab('a')]), '2x4', 'a'), groups = spatialMainGroups(grid.main);
  const empty = groups[1]?.id, occupied = groups[0]?.id;
  if (empty === undefined || occupied === undefined) throw new Error('Expected two cells');
  expect(removeEmptyMainGroup(grid, occupied)).toBe(grid);
  expect(spatialMainGroups(removeEmptyMainGroup(grid, empty).main)).toHaveLength(7);
  const manual: Layout = { ...layout([]), main: { kind: 'split', id: 'manual', direction: 'row', sizes: [50, 50],
    children: [makeGroup([tab('one')]), makeGroup([tab('two')])] } };
  expect(mainGroups(close(manual, 'one').main)).toHaveLength(1);
});

it('plans exact group/main scopes and closes initial/legacy-kept tabs without touching inspectors', () => {
  const initial = { ...tab('initial'), keep: true }, other = tab('other'), second = tab('second');
  const before = layout([]);
  before.main = { kind: 'split', id: 'split', direction: 'row', sizes: [50, 50], children: [
    { kind: 'group', id: 'first', active: 'other', tabs: [initial, other] },
    { kind: 'group', id: 'second-group', active: 'second', tabs: [second] },
  ] };
  expect(tabsForClose(before, { kind: 'group-others', tab: 'initial' })).toEqual([other]);
  expect(tabsForClose(before, { kind: 'group', group: 'second-group' })).toEqual([second]);
  expect(tabsForClose(before, { kind: 'main-others', tab: 'other' })).toEqual([initial, second]);
  expect(tabsForClose(before, { kind: 'main-others', tab: 'files-a' })).toEqual([]);
  const all = tabsForClose(before, { kind: 'main' }); expect(all).toEqual([initial, other, second]);
  const closed = closeTabs(before, new Set(all.map(one => one.id)));
  expect(mainGroups(closed.main).flatMap(group => group.tabs)).toEqual([]);
  expect(closed.left).toEqual(before.left); expect(closed.right).toEqual(before.right); expect(closed.bottom).toEqual(before.bottom);
  expect(mainGroups(before.main).flatMap(group => group.tabs)).toEqual(all);
});

it('bulk close selects the next surviving neighbor, otherwise the previous one', () => {
  const before = layout(['a', 'b', 'c', 'd'].map(tab));
  if (before.main.kind !== 'group') throw new Error('Expected one group'); before.main.active = 'b';
  expect(mainGroups(closeTabs(before, new Set(['b', 'c'])).main)[0]?.active).toBe('d');
  expect(mainGroups(closeTabs(before, new Set(['b', 'c', 'd'])).main)[0]?.active).toBe('a');
});

it('adds duplicate resource instances in the focused group without replacing another tab or its identity', () => {
  const before = applyGrid(layout([tab('a')]), '2x4', 'a'), destination = spatialMainGroups(before.main)[3];
  if (destination === undefined) throw new Error('The requested cell exists');
  const duplicate: Tab = { ...tab('copy'), of: { kind: 'thread', id: 'a' } };
  const next = addMainTab(before, duplicate, destination.id);
  expect(findTab(next, 'copy')?.group.id).toBe(destination.id); expect(findTab(next, 'copy')?.tab).toBe(duplicate);
  expect(findTab(next, 'a')?.tab).toBe(findTab(before, 'a')?.tab);
  expect(spatialMainGroups(next.main)).toHaveLength(8);
});

it('migrates a version-two split layout without changing valid targets, sizes or identities', () => {
  const old = { ...layout([tab('a')]), version: 2 }, restored = parse(serialize(old));
  expect(restored).toEqual({ ...old, version: LAYOUT_VERSION });
});
