/** Persistent tabs are presentation only; navigation, closing and recovery preserve their targets. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { activate, allTabs, applyGrid, findTab, findView, makeContainer, makeGroup, makeTab, move, spatialMainGroups } from '@/features/workbench/layout';
import { moveScoped } from '@/features/workbench/panelScope';
import { loadWorkspace, saveKept } from '@/features/workbench/persistence';
import { keepPresentation, readPresentation } from '@/features/workbench/presentation';
import { useWorkbench } from '@/features/workbench/store';
import { viewKey } from '@/features/workbench/viewIdentity';

const state = () => useWorkbench.getState();
beforeEach(() => { state().leave(); localStorage.clear(); state().enter('workspace-reader', 'thread'); });
afterEach(() => { state().leave(); localStorage.clear(); });
const open = (id: string, mode: 'normal' | 'new' | 'beside' = 'normal') => state().openTab('page', { href: `/thread/${id}` }, mode);

it('preserves the same workspace across page kinds and adds ordinary tabs to the focused main group', () => {
  const a = open('a');
  const b = open('b', 'beside');
  const layout = state().layout;
  state().enter('workspace-reader', 'document');
  expect(state().layout).toBe(layout);
  const c = open('c');
  expect(findTab(state().layout, c)?.group.id).toBe(findTab(state().layout, b)?.group.id);
  expect(findTab(state().layout, a)).not.toBeNull();
  expect(open('a')).toBe(a);
  expect(state().focusedTab).toBe(a);
});

it('gives explicit duplicate resources independent instance identities and preserves them through moves', () => {
  const a = open('a'), duplicate = open('a', 'new');
  expect(duplicate).not.toBe(a);
  const one = findTab(state().layout, a)!.tab, two = findTab(state().layout, duplicate)!.tab;
  expect(one.of).toEqual(two.of);
  expect(viewKey(one)).not.toBe(viewKey(two));
  state().change((layout) => move(layout, a, { to: 'split', group: findTab(layout, duplicate)!.group.id, side: 'down' }));
  expect(findTab(state().layout, a)?.tab).toBe(one);
  expect(viewKey(findTab(state().layout, a)!.tab)).toBe(viewKey(one));
});

it('opening beside keeps the addressed group’s current conversation visible', () => {
  const a = open('a'); open('b'); state().focus(a);
  const duplicate = open('a', 'beside');
  expect(findTab(state().layout, a)?.group.active).toBe(a);
  expect(findTab(state().layout, duplicate)?.group.active).toBe(duplicate);
  expect(state().activeContent).toBe(duplicate);
});

it('keeps the focused content when a fixed inspector receives focus and when another conversation closes', () => {
  const a = open('a'), b = open('b');
  const files = state().openTab('files', { thread: 'a' }, 'new');
  state().change((layout) => ({ ...layout, main: makeGroup(allTabs(layout).filter((tab) => [a, b, files].includes(tab.id)).map((tab) =>
    tab.id === files ? { ...tab, scope: { mode: 'fixed' as const, target: { kind: 'thread' as const, id: 'a' } } } : tab), files) }));
  state().focus(b); state().focus(files);
  expect(state().activeContent).toBe(b);
  state().close({ kind: 'tab', tab: a });
  expect(findTab(state().layout, files)?.tab.scope).toEqual({ mode: 'fixed', target: { kind: 'thread', id: 'a' } });
  expect(state().activeContent).toBe(b);
});

it('closes every main tab, retains grid placeholders and sidebars, and stays empty on reload', () => {
  open('a'); open('b');
  state().change((layout) => applyGrid(layout, '3x3', state().focusedTab));
  const right = state().layout.right;
  state().close({ kind: 'main' });
  expect(spatialMainGroups(state().layout.main)).toHaveLength(9);
  expect(spatialMainGroups(state().layout.main).every((group) => group.tabs.length === 0)).toBe(true);
  expect(state().layout.right).toEqual(right);
  expect(state().activeContent).toBeNull();
  state().leave(); state().enter('workspace-reader', 'thread');
  expect(spatialMainGroups(state().layout.main)).toHaveLength(9);
  expect(spatialMainGroups(state().layout.main).flatMap((group) => group.tabs)).toEqual([]);
  state().reopen();
  expect(state().activeContent).not.toBeNull();
});

it('selects the next neighbor after close, then the next occupied group, preserving close scope', () => {
  state().change((layout) => ({ ...layout, main: makeGroup() }));
  const a = open('a'), b = open('b'), c = open('c');
  const d = open('d', 'beside');
  state().focus(b); state().close({ kind: 'tab', tab: b });
  expect(state().focusedTab).toBe(c);
  state().close({ kind: 'group-others', tab: a });
  expect(findTab(state().layout, d)).not.toBeNull();
  expect(state().focusedTab).toBe(a);
  state().close({ kind: 'group', group: findTab(state().layout, a)!.group.id });
  expect(state().focusedTab).toBe(d);
});

it('asks once for actual unsaved loss and cancellation leaves the complete bulk selection open', () => {
  const a = open('a'), b = open('b');
  state().setUnsaved(b, true);
  const before = state().layout;
  state().close({ kind: 'main' });
  expect(state().pendingClose).toEqual(expect.arrayContaining([a, b]));
  expect(state().layout).toBe(before);
  state().cancelClose();
  expect(state().pendingClose).toBeNull();
  expect(state().layout).toBe(before);
});

it('reset consolidates all ordinary main tabs, retains dirty state and persists immediately', () => {
  const a = open('a'), b = open('b', 'beside');
  state().setUnsaved(a, true);
  state().change((layout) => applyGrid(layout, '2x4', b));
  state().setFocusMode(true); state().reset();
  const groups = spatialMainGroups(state().layout.main);
  expect(groups).toHaveLength(1);
  expect(groups[0]!.tabs.map((tab) => tab.id)).toEqual(expect.arrayContaining([a, b]));
  expect(state().unsaved.has(a)).toBe(true);
  expect(state().focusMode).toBe(false);
  expect(loadWorkspace('workspace-reader', 'page').kept?.layout).toEqual(state().layout);
});

it('focus mode persists separately from bar sizes and visibility', () => {
  const a = open('a');
  const bars = { left: state().layout.left, right: state().layout.right, bottom: state().layout.bottom };
  state().setFocusMode(true);
  state().leave(); state().enter('workspace-reader', 'page');
  expect(state().focusMode).toBe(true);
  expect(state().focusedTab).toBe(a);
  state().setFocusMode(false);
  expect({ left: state().layout.left, right: state().layout.right, bottom: state().layout.bottom }).toEqual(bars);
});

it('migrates valid targets from legacy arrangements once, and isolates signed-in people', () => {
  state().leave(); localStorage.clear();
  const frame = state().layout;
  const a = makeTab('file', { thread: 'a', path: 'report.txt' }), b = makeTab('page', { href: '/doc/b' });
  saveKept('legacy', 'thread', { arrangement: 'conversation', layout: { ...frame, main: makeGroup([a]) } });
  saveKept('legacy', 'document', { arrangement: 'default', layout: { ...frame, main: makeGroup([b]) } });
  state().enter('legacy', 'thread');
  expect(allTabs(state().layout).map((tab) => tab.id)).toEqual([a.id, b.id]);
  state().leave(); state().enter('legacy', 'document');
  expect(allTabs(state().layout).map((tab) => tab.id)).toEqual([a.id, b.id]);
  state().enter('another-person', 'thread');
  expect(allTabs(state().layout).some((tab) => tab.id === a.id || tab.id === b.id)).toBe(false);
});

it('retains the main insertion origin after bottom-inspector focus, including an empty grid cell', () => {
  const a = open('a'), b = open('b', 'beside');
  const group = findTab(state().layout, b)!.group.id;
  const terminal = makeTab('terminal');
  state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, group: makeGroup([terminal]) } }));
  state().focus(terminal.id);
  const c = open('c');
  expect(findTab(state().layout, c)?.group.id).toBe(group);
  state().focus(terminal.id);
  state().close({ kind: 'main-others', tab: state().focusedMainTab! });
  expect(findTab(state().layout, a)).toBeNull();
  expect(findTab(state().layout, c)).not.toBeNull();
  expect(findTab(state().layout, terminal.id)).not.toBeNull();
  state().change((layout) => applyGrid(layout, '3x3', c));
  const empty = spatialMainGroups(state().layout.main).at(-1)!;
  state().setFocusedGroup(empty.id); state().focus(terminal.id);
  const d = open('d');
  expect(findTab(state().layout, d)?.group.id).toBe(empty.id);
});

it.each(['reset', 'conversation', 'code', 'review'])('keeps the visible main selection and content context consistent during %s from an inspector', (choice) => {
  state().change((layout) => ({ ...layout, main: makeGroup() }));
  const a = open('a'), b = open('b');
  const terminal = makeTab('terminal');
  state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, group: makeGroup([terminal]) } }));
  state().focus(terminal.id);
  if (choice === 'reset') state().reset(); else state().choose(choice);
  expect(findTab(state().layout, b)?.group.active).toBe(b);
  expect(state().activeContent).toBe(b);
  expect(state().focusedMainTab).toBe(b);
  expect(findTab(state().layout, a)).not.toBeNull();
});

it('opening a following panel does not retarget or consume an existing fixed panel', () => {
  const fixed = makeTab('files');
  fixed.scope = { mode: 'fixed', target: { kind: 'thread', id: 'a' } };
  state().change((layout) => ({ ...layout, main: makeGroup([fixed]) }));
  expect(findView(state().layout, 'files')).toBeNull();
  expect(findView(state().layout, 'files', undefined, fixed.scope)?.tab.id).toBe(fixed.id);
  const following = state().openTab('files', undefined);
  expect(following).not.toBe(fixed.id);
  expect(findTab(state().layout, fixed.id)?.tab.scope).toEqual(fixed.scope);
  expect(findView(state().layout, 'files')?.tab.id).toBe(following);
});

it.each(['heading', 'body'] as const)('persists moved Chat selection after sidebar %s focus and resets without consuming its conversation or scroll state', (source) => {
  state().change((layout) => ({ ...layout, main: makeGroup() }));
  const a = open('a'), chat = makeTab('chat'), outline = makeTab('outline');
  const container = makeContainer('conversation', [chat, outline]);
  state().change((layout) => ({ ...layout, right: { ...layout.right, open: true, active: container.name, containers: [container] } }));
  keepPresentation(a, 'scroll', { top: 1 });
  // A header focus does not enter the body slot; both menu and drag movement carry the addressed tab explicitly.
  if (source === 'body') state().focus(chat.id);
  state().change((layout) => moveScoped(layout, chat.id, { to: 'area', area: 'main' }), chat.id);
  expect(state().focusedTab).toBe(chat.id); expect(state().focusedMainTab).toBe(chat.id); expect(state().activeContent).toBe(a);
  const fixed = findTab(state().layout, chat.id)!.tab;
  expect(fixed.scope).toMatchObject({ mode: 'fixed', target: { kind: 'thread', id: 'a' } });
  state().focus(outline.id); state().flush(); state().leave(); state().enter('workspace-reader', 'thread');
  expect(state().focusedTab).toBe(outline.id); expect(state().focusedMainTab).toBe(chat.id); expect(state().activeContent).toBe(a);
  expect(findTab(state().layout, chat.id)?.group.active).toBe(chat.id);
  state().reset();
  expect(spatialMainGroups(state().layout.main)).toHaveLength(1);
  expect(findTab(state().layout, chat.id)?.group.active).toBe(chat.id);
  expect(findTab(state().layout, chat.id)?.tab).toEqual(fixed);
  expect(findTab(state().layout, a)?.tab.of).toEqual({ href: '/thread/a' });
  expect(readPresentation(a, 'scroll')).toEqual({ top: 1 });
  expect(state().activeContent).toBe(a);
});

it('keeps a moved fixed inspector selected in grid overflow and picks its content neighbor after closing it from sidebar focus', () => {
  state().change((layout) => ({ ...layout, main: makeGroup() }));
  const content = Array.from({ length: 9 }, (_, index) => open(`thread-${index}`));
  const current = content.at(-1)!;
  const chat = makeTab('chat'), outline = makeTab('outline'), container = makeContainer('conversation', [chat, outline]);
  state().change((layout) => ({ ...layout, right: { ...layout.right, open: true, active: container.name, containers: [container] } }));
  state().change((layout) => moveScoped(layout, chat.id, { to: 'area', area: 'main' }), chat.id);
  state().focus(outline.id);
  state().change((layout) => applyGrid(layout, '2x4', state().focusedMainTab));
  const cells = spatialMainGroups(state().layout.main);
  expect(cells).toHaveLength(8); expect(cells[7]?.tabs.map((tab) => tab.id)).toEqual([...content.slice(7), chat.id]);
  expect(cells[7]?.active).toBe(chat.id); expect(state().focusedMainTab).toBe(chat.id); expect(state().activeContent).toBe(current);
  state().close({ kind: 'tab', tab: chat.id });
  expect(state().focusedTab).toBe(outline.id); expect(state().focusedMainTab).toBe(current);
  expect(findTab(state().layout, current)?.group.active).toBe(current); expect(state().activeContent).toBe(current);
  expect(spatialMainGroups(state().layout.main).flatMap((group) => group.tabs.map((tab) => tab.id))).toEqual(content);
});

it('retains focus when a layout edit changes another group and carries content focus only when that movement is explicit', () => {
  state().change((layout) => ({ ...layout, main: makeGroup() }));
  const a = open('a'), b = open('b', 'beside'), c = open('c');
  state().focus(b); state().focus(a);
  state().change((layout) => activate(layout, c));
  expect(findTab(state().layout, c)?.group.active).toBe(c);
  expect(state().focusedTab).toBe(a); expect(state().focusedMainTab).toBe(a); expect(state().activeContent).toBe(a);
  state().change((layout) => move(layout, c, { to: 'group', group: findTab(layout, a)!.group.id }), c);
  expect(state().focusedTab).toBe(c); expect(state().focusedMainTab).toBe(c); expect(state().activeContent).toBe(c);
  expect(findTab(state().layout, c)?.group.active).toBe(c); expect(findTab(state().layout, b)).not.toBeNull();
});
