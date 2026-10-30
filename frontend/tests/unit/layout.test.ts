/** Observable workbench choices survive changes, device storage and the upgrade from tab bars. */
import { beforeAll, describe, expect, it } from 'vitest';

import { ARRANGEMENTS, shippedLayout } from '@/features/workbench/Arrangements';
import {
  activeContainer, allTabs, canMove, carryKept, close, expandSection,
  findTab, findView, keepOffered, type Layout, LAYOUT_VERSION, mainGroups,
  makeGroup, move, normalize, open, parse, resizeBar, resizeSections,
  resizeSplit, selectContainer, serialize, setBarOpen, setKept, showSection,
  toggleBar, toggleMaximized,
} from '@/features/workbench/layout';
import { loadKept, saveKept } from '@/features/workbench/persistence';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import { useWorkbench } from '@/features/workbench/store';
import { readViewChoice } from '@/lib/viewChoices';
import '@/features/chat/register';
import '@/features/documents/register';
import '@/features/navigation/Resources';
import { registerInspectorViews } from '@/features/inspector/Inspector';

beforeAll(() => { registerShippedViews(() => null); registerInspectorViews(); });
const SIZES = { left: 280, right: 400, bottom: 240 };
const thread = (): Layout => shippedLayout('thread', 'conversation', SIZES);
const tabOf = (layout: Layout, view: string): string => {
  const place = findView(layout, view);
  if (place === null) throw new Error(`no ${view} in the layout`);
  return place.tab.id;
};
const mainViews = (layout: Layout) => mainGroups(layout.main).map((group) => group.tabs.map((tab) => tab.view));
const viewsOf = (layout: Layout, bar: 'left' | 'right') => layout[bar].containers.flatMap((container) => container.sections.map((section) => section.tab.view));

it('ships related sections in five thread containers and keeps the terminal below', () => {
  const layout = thread();
  expect(layout.version).toBe(LAYOUT_VERSION);
  expect(layout.right.containers.map((container) => [container.name, container.sections.map((section) => section.tab.view)])).toEqual([
    ['conversation', ['outline', 'history']], ['team', ['chat', 'documents', 'people']],
    ['code', ['changes', 'files', 'git']], ['environment', ['environment', 'processes', 'resources', 'ports', 'browsers', 'displays']],
    ['agent', ['parameters', 'usage', 'knowledge', 'given', 'diagnostics']],
  ]);
  expect(mainViews(layout)).toEqual([['screen']]);
  expect(layout.right.active).toBe('conversation');
  expect(layout.bottom.group.tabs.map((tab) => tab.view)).toEqual(['terminal']);
  expect(layout.bottom.open).toBe(false);
});

it('offers the same views in each thread arrangement, once each', () => {
  for (const arrangement of ARRANGEMENTS.thread) {
    const layout = shippedLayout('thread', arrangement.name, SIZES);
    expect(allTabs(layout).map((tab) => tab.view).sort()).toEqual(allTabs(thread()).map((tab) => tab.view).sort());
  }
  expect(mainViews(shippedLayout('thread', 'code', SIZES))).toEqual([['files']]);
  expect(mainViews(shippedLayout('thread', 'review', SIZES))).toEqual([['changes']]);
  expect(viewsOf(shippedLayout('document', undefined, SIZES), 'right')).toEqual(['comments', 'outline', 'links']);
  expect(shippedLayout('topic', undefined, SIZES).right.containers.map((container) => container.name)).toEqual(['team', 'environment']);
});

it('reveals a hidden, collapsed section in its own container without duplicating it', () => {
  const before = thread();
  const chat = tabOf(before, 'chat');
  const hidden = setBarOpen(showSection(expandSection(before, chat, false), chat, false), 'right', false);
  const after = open(hidden, 'chat', 'right');
  expect(after.right.open).toBe(true);
  expect(after.right.active).toBe('team');
  expect(findTab(after, chat)?.section).toMatchObject({ open: true, hidden: false });
  expect(allTabs(open(after, 'chat', 'right')).filter((tab) => tab.view === 'chat')).toHaveLength(1);
  expect(findTab(before, chat)?.section?.hidden).toBe(false);
});

it('hides a section while retaining its order, expansion and resized share', () => {
  const before = thread();
  const chat = tabOf(before, 'chat');
  const resized = resizeSections(before, 'right', 'team', { [chat]: 3 });
  const hidden = close(expandSection(resized, chat, false), chat);
  expect(findTab(hidden, chat)?.section).toMatchObject({ hidden: true, open: false, size: 3 });
  const shown = showSection(hidden, chat, true);
  expect(findTab(shown, chat)?.section).toMatchObject({ hidden: false, open: false, size: 3 });
  expect(viewsOf(shown, 'right')).toEqual(viewsOf(before, 'right'));
});

it('switches one activity container and toggles it closed and open', () => {
  const layout = selectContainer(thread(), 'left', 'resource-navigation', true);
  expect(activeContainer(layout.left)?.name).toBe('resource-navigation');
  expect(layout.left.open).toBe(true);
  const hidden = selectContainer(layout, 'left', 'resource-navigation', true);
  expect(hidden.left.open).toBe(false);
  expect(selectContainer(hidden, 'left', 'resource-navigation', true).left.open).toBe(true);
  expect(selectContainer(hidden, 'left', 'navigation', true).left.active).toBe('navigation');
});

it('moves a section to another container, then the bottom and main, preserving its identity and target', () => {
  const before = open(thread(), 'file', 'main', { path: 'src/main.rs', thread: 'one' });
  const file = findView(before, 'file', { path: 'src/main.rs', thread: 'one' })?.tab;
  if (file === undefined) throw new Error('file did not open');
  const side = move(before, file.id, { to: 'container', bar: 'right', container: 'team', index: 1 });
  expect(findTab(side, file.id)?.container?.name).toBe('team');
  expect(side.right.active).toBe('team');
  expect(findTab(side, file.id)?.index).toBe(1);
  const bottom = move(side, file.id, { to: 'area', area: 'bottom' });
  expect(bottom.bottom.open).toBe(true);
  expect(bottom.bottom.group.active).toBe(file.id);
  const main = move(bottom, file.id, { to: 'area', area: 'main' });
  expect(findTab(main, file.id)?.area).toBe('main');
  expect(findTab(main, file.id)?.tab).toEqual(file);
  expect(allTabs(main).filter((tab) => tab.id === file.id)).toHaveLength(1);
});

it('reorders sections, splits and rejoins main groups and gives one group the whole area', () => {
  const before = thread();
  const outline = tabOf(before, 'outline');
  const reordered = move(before, tabOf(before, 'history'), { to: 'container', bar: 'right', container: 'conversation', index: 0 });
  expect(reordered.right.containers[0]?.sections[0]?.tab.view).toBe('history');
  const first = mainGroups(before.main)[0]?.id ?? '';
  let layout = move(before, outline, { to: 'split', group: first, side: 'right' });
  expect(mainViews(layout)).toEqual([['screen'], ['outline']]);
  expect(canMove(layout, outline, { to: 'split', group: findTab(layout, outline)?.group.id ?? '', side: 'down' })).toBe(false);
  layout = toggleMaximized(layout, first);
  expect(layout.maximized).toBe(first);
  if (layout.main.kind !== 'split') throw new Error('no split');
  expect(resizeSplit(layout, layout.main.id, [30, 90]).main).toMatchObject({ sizes: [25, 75] });
  expect(move(layout, outline, { to: 'group', group: first }).maximized).toBeNull();
});

it('closes a main tab and selects its neighbour, leaving a place to drop into when empty', () => {
  const before = open(thread(), 'file', 'main', { path: 'a' });
  const file = findView(before, 'file', { path: 'a' })?.tab.id ?? '';
  expect(mainViews(close(before, file))).toEqual([['screen']]);
  const base = thread();
  expect(mainViews(move(base, tabOf(base, 'screen'), { to: 'area', area: 'bottom' }))).toEqual([[]]);
  expect(base.version).toBe(LAYOUT_VERSION);
});

it('keeps a selected tab and carries it through navigation only once', () => {
  const before = thread();
  const kept = setKept(before, tabOf(before, 'parameters'), true);
  const arrived = carryKept(kept, shippedLayout('project', undefined, SIZES));
  expect(mainViews(arrived)).toEqual([['screen', 'parameters']]);
  expect(allTabs(carryKept(kept, arrived)).filter((tab) => tab.view === 'parameters')).toHaveLength(1);
});

it('resizes a bar in whole pixels and refuses an empty bar opening', () => {
  expect(resizeBar(thread(), 'left', 312.6).left.size).toBe(313);
  const empty = shippedLayout('page', undefined, SIZES);
  expect(toggleBar(empty, 'bottom')).toBe(empty);
  expect(resizeBar(empty, 'left', 0)).toBe(empty);
});

/** Explicitly rearranged old layout: files on the left, chat below, a kept file beside the route. */
const oldLayout = () => ({
  version: 1,
  left: { open: true, size: 333, group: { kind: 'group', id: 'old-left', active: 'files', tabs: [{ id: 'files', view: 'files' }, { id: 'nav', view: 'navigation' }] } },
  right: { open: false, size: 367, group: { kind: 'group', id: 'old-right', active: 'changes', tabs: [{ id: 'history', view: 'history' }, { id: 'changes', view: 'changes' }, { id: 'extension', view: 'an-extension', keep: true }] } },
  bottom: { open: true, size: 211, group: { kind: 'group', id: 'old-bottom', active: 'chat', tabs: [{ id: 'chat', view: 'chat' }] } },
  main: { kind: 'split', id: 'old-split', direction: 'row', sizes: [35, 65], children: [
    { kind: 'group', id: 'route', active: 'screen', tabs: [{ id: 'screen', view: 'screen' }] },
    { kind: 'group', id: 'editor', active: 'file', tabs: [{ id: 'file', view: 'file', of: { path: 'src/main.rs', thread: 'one' }, keep: true }] },
  ] }, maximized: 'editor',
});

describe('device storage', () => {
  it('reads old choices into containers without losing any tab, target, bar, split, active choice or size', () => {
    const old = oldLayout();
    const layout = parse(JSON.stringify(old), 'thread');
    if (layout === null) throw new Error('old layout refused');
    expect(layout.version).toBe(LAYOUT_VERSION);
    expect(layout.main).toEqual(old.main);
    expect(layout.maximized).toBe('editor');
    expect(layout.bottom).toEqual(old.bottom);
    expect(layout.left).toMatchObject({ size: 333, open: true, active: 'code' });
    expect(layout.right).toMatchObject({ size: 367, open: false, active: 'code' });
    for (const id of ['files', 'nav', 'history', 'changes', 'extension', 'chat', 'screen', 'file']) expect(findTab(layout, id)).not.toBeNull();
    expect(findTab(layout, 'files')?.area).toBe('left');
    expect(findTab(layout, 'changes')?.section?.open).toBe(true);
    expect(parse(serialize(layout))).toEqual(layout);
    localStorage.setItem('orch.migration.workbench.thread', JSON.stringify({ arrangement: 'code', layout: JSON.stringify(old) }));
    expect(loadKept('migration', 'thread').dropped).toBe(false);
  });

  it('retains hidden sections, expansion, relative sizes and another-container moves on a round trip', () => {
    let layout = thread();
    const chat = tabOf(layout, 'chat');
    layout = move(layout, chat, { to: 'container', bar: 'right', container: 'code' });
    layout = resizeSections(layout, 'right', 'code', { [chat]: 7 });
    layout = showSection(expandSection(layout, chat, false), chat, false);
    saveKept('reader', 'thread', { arrangement: 'conversation', layout });
    expect(loadKept('reader', 'thread')).toEqual({ kept: { arrangement: 'conversation', layout }, dropped: false });
    expect(loadKept('someone-else', 'thread').kept).toBeNull();
    expect(loadKept('reader', 'topic').kept).toBeNull();
  });

  it('resets choices to the shipped arrangement and forgets the stored one', () => {
    useWorkbench.getState().leave();
    useWorkbench.getState().enter('reset-reader', 'thread');
    useWorkbench.getState().change((layout) => move(layout, tabOf(layout, 'chat'), { to: 'area', area: 'bottom' }));
    useWorkbench.getState().enter('reset-reader', 'page');
    useWorkbench.getState().enter('reset-reader', 'thread');
    expect(findView(useWorkbench.getState().layout, 'chat')?.area).toBe('bottom');
    useWorkbench.getState().reset();
    expect(findView(useWorkbench.getState().layout, 'chat')?.container?.name).toBe('team');
    expect(readViewChoice('reset-reader', 'global', 'rightOpen', false)).toBe(true);
    expect(readViewChoice('reset-reader', 'global', 'mobileContainer', 'old')).toBe('');
    expect(loadKept('reset-reader', 'thread').kept).toBeNull();
  });

  it('rejects corrupt and future shapes and reports the reset once', () => {
    const layout = thread();
    expect(parse(JSON.stringify({ ...layout, version: 99 }))).toBeNull();
    expect(parse(JSON.stringify({ ...layout, left: { open: true } }))).toBeNull();
    const first = allTabs(layout)[0];
    expect(parse(JSON.stringify({ ...layout, bottom: { ...layout.bottom, group: makeGroup(first === undefined ? [] : [first]) } }))).toBeNull();
    expect(parse('null')).toBeNull();
    expect(parse('{')).toBeNull();
    localStorage.setItem('orch.corrupt.workbench.thread', 'broken');
    expect(loadKept('corrupt', 'thread')).toEqual({ kept: null, dropped: true });
    expect(loadKept('corrupt', 'thread')).toEqual({ kept: null, dropped: false });
  });

  it('projects unavailable views without mutating the saved choices', () => {
    const before = thread();
    const read = keepOffered(before, (view) => view !== 'chat');
    expect(findView(read, 'chat')).toBeNull();
    expect(findView(before, 'chat')).not.toBeNull();
    expect(normalize(read).version).toBe(LAYOUT_VERSION);
  });
});

// The chooser's dynamic catalogue registration is exercised through the actual product reader.
it('gives Links its registered bilingual chooser words', async () => {
  const { viewOf } = await import('@/features/workbench/views');
  const { viewChoice } = await import('@/features/workbench/choiceWords');
  const { i18n } = await import('@/i18n');
  const view = viewOf('links');
  if (view === undefined) throw new Error('Links must be registered in the native chooser.');
  expect(viewChoice(view, i18n.t)).toEqual({
    name: i18n.t('workbench.choices.views.links.name'),
    description: i18n.t('workbench.choices.views.links.description'),
  });
});
