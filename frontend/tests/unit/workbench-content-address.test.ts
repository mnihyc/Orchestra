/** Full content resource identities survive URLs, explicit clones and supported old layouts. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { contentAddress, contentHref, contentLocation, contentTab, contentTarget, parseContentAddress, type ContentLocation } from '@/features/workbench/contentAddress';
import { openContent } from '@/features/workbench/contentNavigation';
import { allTabs, applyGrid, findTab, makeGroup, makeTab, move, parse, serialize } from '@/features/workbench/layout';
import { panelTarget, moveScoped } from '@/features/workbench/panelScope';
import { useWorkbench } from '@/features/workbench/store';
import { offers, type ViewDefinition } from '@/features/workbench/views';
import { File } from 'lucide-react';

const state = () => useWorkbench.getState();
const file: ContentLocation = { view: 'file', environmentId: 'environment-a', threadId: 'thread-a', path: 'src/空间 #1?%.rs', line: 41 };
const differences: ContentLocation[] = [
  { view: 'difference', environmentId: 'environment-a', threadId: 'thread-a', path: 'src/main.rs', kind: 'git', item: '', staged: true },
  { view: 'difference', environmentId: 'environment-a', threadId: null, path: 'src/main.rs', kind: 'change', item: 'actual-item', staged: false },
];
beforeEach(() => { state().leave(); localStorage.clear(); state().enter('content-address-reader', 'thread'); });
afterEach(() => { state().leave(); localStorage.clear(); });

it.each([file, ...differences, { view: 'page', href: '/thread/thread-a?continuation=version-b&item=item-2&prompt=input-2' } satisfies ContentLocation])('round trips the complete $view source without any local owner in its shared URL', (location) => {
  const href = contentHref(location);
  const one = makeTab(contentTab(location).view, contentTab(location).of);
  const two = makeTab(contentTab(location).view, contentTab(location).of);
  expect(one.id).not.toBe(two.id);
  expect(contentAddress(one)).toBe(href);
  expect(parseContentAddress(href)).toEqual(location);
  expect(contentLocation(two)).toEqual(location);
  expect(href).not.toContain(one.id);
  expect(href).not.toContain(two.id);
});

it.each(['123', 'true', '[1,2]'])('preserves the exact JSON-looking filename %s through router search serialization and raw old links', (path) => {
  const location: ContentLocation = { view: 'file', environmentId: 'environment-a', threadId: 'thread-a', path };
  const href = contentHref(location);
  expect(parseContentAddress(href)).toEqual(location);
  expect(contentHref(parseContentAddress(href)!)).toBe(href);
  expect(parseContentAddress(`/workbench?view=file&environment=environment-a&thread=thread-a&path=${encodeURIComponent(path)}`)).toEqual(location);
});

it.each([
  '/workbench?view=file&environment=a&path=../secret',
  '/workbench?view=file&environment=a&path=/secret',
  '/workbench?view=file&environment=a&path=src%2F..%2Fsecret',
  '/workbench?view=file&environment=a&environment=b&path=main.rs',
  '/workbench?view=file&environment=a&thread=&path=main.rs',
  '/workbench?view=file&environment=a&path=main.rs&line=0',
  '/workbench?view=file&environment=a&path=main.rs&line=9007199254740992',
  '/workbench?view=difference&environment=a&path=main.rs&kind=git&item=foreign-item',
  '/workbench?view=difference&environment=a&path=main.rs&kind=change&staged=1',
  '//outside.example.test/workbench?view=file&environment=a&path=main.rs',
])('refuses the malformed locator without a fallback source: %s', (href) => {
  const before = state().layout;
  expect(parseContentAddress(href)).toBeNull();
  expect(openContent(href)).toBeNull();
  expect(state().layout).toBe(before);
});

it('file-only focus has its own workspace target and survives close, reopen, frame/reset and reload', () => {
  const page = state().openTab('page', { href: '/thread/thread-b' });
  const id = openContent(file);
  if (id === null) throw new Error('The complete file locator must open.');
  state().focus(page); state().focus(id);
  expect(state().activeContent).toBe(id);
  const target = contentTarget(findTab(state().layout, id)!.tab);
  expect(target).toEqual({ kind: 'environment', id: 'environment-a', workspace: { environmentId: 'environment-a', threadId: 'thread-a' } });
  expect(panelTarget(makeTab('files'), state().layout, state().activeContent, null)).toEqual(target);
  const fixed = makeTab('files'); fixed.scope = { mode: 'fixed', target: { kind: 'thread', id: 'thread-b' } };
  expect(panelTarget(fixed, state().layout, id, null)).toEqual(fixed.scope.target);
  state().close({ kind: 'tab', tab: page });
  state().change((layout) => applyGrid(layout, '3x3', id)); state().reset(); state().flush();
  expect(contentAddress(findTab(state().layout, id)!.tab)).toBe(contentHref(file));
  state().leave(); state().enter('content-address-reader', 'page');
  expect(state().activeContent).toBe(id);
  state().close({ kind: 'tab', tab: id });
  expect(state().activeContent).toBeNull();
  state().reopen();
  expect(contentAddress(findTab(state().layout, id)!.tab)).toBe(contentHref(file));
});

it('normal resource reuse preserves peers while new/beside clones the exact native target with distinct local owners', () => {
  const first = openContent(file);
  const other = openContent(differences[0]!);
  const again = openContent({ ...file, line: 72 });
  expect(again).toBe(first);
  expect(findTab(state().layout, first!)?.tab.of?.line).toBe('72');
  const duplicate = openContent(file, 'new'), beside = openContent(file, 'beside');
  expect(new Set([first, duplicate, beside]).size).toBe(3);
  expect(contentLocation(findTab(state().layout, duplicate!)!.tab)).toEqual(file);
  expect(contentLocation(findTab(state().layout, beside!)!.tab)).toEqual(file);
  expect(findTab(state().layout, first!)).not.toBeNull();
  expect(findTab(state().layout, other!)).not.toBeNull();
});

it.each([1, 2, 3])('migrates every valid main result in layout version %s without replacing identity, group selection or locator', (version) => {
  const one = makeTab('search-result', { kind: 'thread', id: 'thread-a', continuation: 'history-b', prompt: 'input-b', anchor: 'actual-item-b' });
  const document = makeTab('search-result', { kind: 'document', id: 'document-a', anchor: 'block-a', version: '7' });
  const chat = makeTab('search-result', { kind: 'chat', id: 'chat-a', anchor: 'message-a' });
  const placeChat = makeTab('search-result', { kind: 'place-chat', id: 'place-a', anchor: 'message-b' });
  const group = makeGroup([one, document, chat, placeChat], document.id);
  const value = { ...state().layout, version, main: group };
  const legacy = version === 1 ? { ...value, left: { open: false, size: 220, group: makeGroup() }, right: { open: false, size: 320, group: makeGroup() } } : value;
  const loaded = parse(JSON.stringify(legacy));
  expect(loaded).not.toBeNull();
  expect(findTab(loaded!, document.id)?.group.id).toBe(group.id);
  expect(findTab(loaded!, document.id)?.group.active).toBe(document.id);
  expect(allTabs(loaded!).filter((tab) => tab.view === 'page').map((tab) => tab.id)).toEqual([one.id, document.id, chat.id, placeChat.id]);
  expect(contentAddress(findTab(loaded!, one.id)!.tab)).toBe('/thread/thread-a?item=actual-item-b&continuation=history-b&prompt=input-b');
  expect(contentAddress(findTab(loaded!, document.id)!.tab)).toBe('/doc/document-a?block=block-a&version=7');
  expect(contentAddress(findTab(loaded!, chat.id)!.tab)).toBe('/chat/chat-a?message=message-a');
  expect(contentAddress(findTab(loaded!, placeChat.id)!.tab)).toBe('/place-chat/place-a?message=message-b');
  expect(parse(serialize(loaded!))).toEqual(loaded);
});

it('promotes a non-main preview only when it enters main, keeping all other retained tabs and dirty state', () => {
  const main = state().openTab('page', { href: '/thread/other' });
  const preview = makeTab('search-result', { kind: 'document', id: 'document-a', anchor: 'block-a', version: '2' });
  state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, group: makeGroup([preview]), open: true } }));
  expect(findTab(state().layout, preview.id)?.tab).toBe(preview);
  state().setUnsaved(main, true);
  state().change((layout) => move(layout, preview.id, { to: 'split', group: findTab(layout, main)!.group.id, side: 'right' }));
  expect(findTab(state().layout, preview.id)?.tab.view).toBe('page');
  expect(contentAddress(findTab(state().layout, preview.id)!.tab)).toBe('/doc/document-a?block=block-a&version=2');
  expect(state().unsaved.has(main)).toBe(true);
  expect(findTab(state().layout, main)).not.toBeNull();
});

it('fixing a followed workspace keeps its actual source through move and serialized reload', () => {
  const id = openContent(file)!;
  const panel = makeTab('files');
  state().change((layout) => ({ ...layout, right: { ...layout.right, open: true, active: 'code', containers: [{ id: 'fixed-code', name: 'code', active: panel.id, sections: [{ tab: panel, open: true, hidden: false, size: 1 }] }] } }));
  state().change((layout) => moveScoped(layout, panel.id, { to: 'split', group: findTab(layout, id)!.group.id, side: 'right' }));
  const restored = parse(serialize(state().layout));
  expect(findTab(restored!, panel.id)?.tab.scope).toEqual({ mode: 'fixed', target: contentTarget(findTab(state().layout, id)!.tab) });
});

it('the neutral file address still offers compatible panels only from actual current workspace metadata', () => {
  const definition: ViewDefinition = { name: 'files', title: 'views.files.title', about: 'views.files.about', icon: File, screens: ['thread'], area: 'right', body: () => null };
  expect(offers(definition, 'page', { title: 'Known file', target: { kind: 'environment', id: 'workspace-a' }, views: { ready: true, workspace: true } })).toBe(true);
  expect(offers(definition, 'page', { title: 'Other page', views: { ready: true } })).toBe(false);
  expect(offers(definition, 'page', { title: 'Refused source', target: { kind: 'environment', id: 'workspace-a' }, views: { ready: true, workspace: false } })).toBe(false);
  expect(offers({ ...definition, name: 'git' }, 'page', { title: 'No through-thread', target: { kind: 'environment', id: 'workspace-a' }, views: { ready: true, workspace: true } })).toBe(false);
});
