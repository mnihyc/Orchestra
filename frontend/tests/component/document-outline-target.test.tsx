/** Actual Outline actions, workbench targets and per-pane marks; editor DOM/lifecycle is the only rendering seam. */
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createRef, useLayoutEffect, useRef } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Editor } from '@tiptap/core';
import type { StoreApi } from 'zustand';
import { ApiError } from '@/api/client';
import { bindEditorMarks, DocumentMarksProvider, retireDocumentMarks, useDocumentMarksStore, useMarks, type Marks } from '@/features/documents/marks';
import { DocumentOutline } from '@/features/people/DocumentOutline';
import { bindContentNavigation } from '@/features/workbench/contentNavigation';
import { contentAddress, contentTarget, parsePageAddress } from '@/features/workbench/contentAddress';
import { ViewInstanceContext } from '@/features/workbench/instance';
import { allTabs, findTab, makeContainer, makeGroup, makeTab, newId, type Tab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { documentWriter } from '@/realtime/stores/document';
import type { Document, DocumentBlock } from '@/generated/api-types';
import { World } from '../doubles/world';

const state = () => useWorkbench.getState();
const frames: FrameRequestCallback[] = [], stores = new Map<string, StoreApi<Marks>>();
const scrolls: { owner: string | null; block: string | null }[] = [];
let doc: Document, heading: DocumentBlock, fold: DocumentBlock, one: Tab, two: Tab, other: Tab, outline: Tab;
let navigation: ReturnType<typeof vi.fn<Parameters<typeof bindContentNavigation>[0]>>, unbindNavigation: () => void;
function flushFrames() {
  act(() => {
    // One frame after local folds, and (only for a repeated inactive block link) one after activation.
    for (let phase = 0; phase < 2; phase += 1) for (const frame of frames.splice(0)) frame(phase);
  });
}

function EditorDom({ owner, includeHeading = true }: { owner: string; includeHeading?: boolean }) {
  const root = useRef<HTMLDivElement>(null), marks = useDocumentMarksStore();
  const visible = useWorkbench((value) => findTab(value.layout, owner)?.group.active === owner);
  useLayoutEffect(() => {
    if (root.current === null) throw new Error('The addressed editor root is required.');
    stores.set(owner, marks);
    const editor = { isDestroyed: false, view: { dom: root.current } };
    const stop = bindEditorMarks(editor as unknown as Editor, marks);
    return () => { editor.isDestroyed = true; stop(); };
  }, [marks, owner]);
  return <div ref={root} data-editor-owner={owner} hidden={!visible}>
    <div data-id={fold.id}>
      <div data-folded="true"><button className="oc-fold" onClick={(event) => { event.currentTarget.parentElement?.setAttribute('data-folded', 'false'); }}>Fold {owner}</button></div>
      <div>{includeHeading && <div data-id={heading.id}>Owned heading</div>}</div>
    </div>
  </div>;
}
function Pane({ tab, includeHeading = true }: { tab: Tab; includeHeading?: boolean }) {
  const live = useWorkbench((value) => findTab(value.layout, tab.id) !== null);
  return live ? <DocumentMarksProvider owner={tab.id} documentId={doc.id}><EditorDom owner={tab.id} includeHeading={includeHeading} /></DocumentMarksProvider> : null;
}
function Inspector() {
  const layout = useWorkbench((value) => value.layout), active = useWorkbench((value) => value.activeContent);
  const tab = findTab(layout, outline.id)?.tab;
  if (tab === undefined) return null;
  const fixedTarget = tab.scope?.mode === 'fixed' ? tab.scope.target : null;
  const fixed = fixedTarget !== null;
  const content = active === null ? undefined : findTab(layout, active)?.tab;
  const target = fixedTarget ?? (content === undefined ? null : contentTarget(content));
  if (target?.kind !== 'document') return null;
  const owner = fixed ? tab.id : active;
  if (owner === null) return null;
  return <ViewInstanceContext.Provider value={{ id: tab.id, root: createRef(), target, headingOwner: owner, content: false, focusedContent: false }}>
    <DocumentMarksProvider owner={owner} documentId={target.id}><DocumentOutline documentId={target.id} /></DocumentMarksProvider>
  </ViewInstanceContext.Provider>;
}
function Frame({ missingSecond = false }: { missingSecond?: boolean }) { return <><Pane tab={one} /><Pane tab={two} includeHeading={!missingSecond} /><Inspector /></>; }
const button = () => screen.getByRole('button', { name: /Rollout plan/ });
const currentMark = (id: string) => stores.get(id)?.getState().highlight;

beforeEach(() => {
  vi.useFakeTimers(); cleanup(); state().leave(); localStorage.clear(); retireDocumentMarks(); documentWriter.clear();
  frames.length = 0; stores.clear(); scrolls.length = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.push(callback); return frames.length; });
  const css: unknown = Reflect.get(globalThis, 'CSS');
  if (typeof css !== 'object' || css === null || typeof Reflect.get(css, 'escape') !== 'function') vi.stubGlobal('CSS', { escape: (id: string) => {
    // Fixture identifiers are safe inside the quoted attribute selectors; the browser proof uses native CSS.escape.
    if (!/^[a-z0-9-]+$/i.test(id)) throw new Error('The fixture selector shim only accepts its ordinary identifiers.');
    return id;
  } });
  vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(function (this: Element) {
    scrolls.push({ owner: this.closest('[data-editor-owner]')?.getAttribute('data-editor-owner') ?? null, block: this.getAttribute('data-id') });
  });
  const world = new World(), ada = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((value) => value.title === 'Rollout plan');
  if (ada === undefined || stored === undefined) throw new Error('The ordinary readable document fixture is required.');
  doc = world.documents.full(ada, stored);
  const rows = world.documents.sync(ada, stored, null).blocks;
  const parent = rows.find((block) => block.type === 'toggle'), child = rows.find((block) => block.type === 'heading');
  if (parent === undefined || child === undefined) throw new Error('Native fold and heading shapes are required.');
  fold = parent; heading = { ...child, parent_id: parent.id };
  documentWriter.reset(doc.id, { document: doc, blocks: [fold, heading], discussions: [] }, doc.seq);
  state().enter('outline-reader', 'document');
  one = makeTab('page', { href: `/doc/${doc.id}` }); two = makeTab('page', { href: `/doc/${doc.id}` });
  other = makeTab('page', { href: '/doc/another-document' }); outline = makeTab('outline');
  const groups = [makeGroup([one]), makeGroup([two])], container = makeContainer('document', [outline]);
  state().change((layout) => ({ ...layout, main: { kind: 'split', id: newId('s'), direction: 'row', sizes: [50, 50], children: groups },
    right: { ...layout.right, open: true, active: container.name, containers: [container] } }));
  state().focus(two.id);
  navigation = vi.fn<Parameters<typeof bindContentNavigation>[0]>(); unbindNavigation = bindContentNavigation(navigation);
});
afterEach(() => {
  cleanup(); unbindNavigation(); state().leave(); retireDocumentMarks(); documentWriter.clear(); localStorage.clear();
  vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers();
});

it('Follow Outline on the second of two identical documents unfolds, scrolls and highlights only that second editor', () => {
  render(<Frame />);
  fireEvent.click(button()); flushFrames();
  expect(scrolls).toEqual([{ owner: two.id, block: heading.id }]);
  expect(currentMark(two.id)).toBe(heading.id); expect(currentMark(one.id)).toBeNull(); expect(useMarks.getState().highlight).toBeNull();
  expect(document.querySelector(`[data-editor-owner="${two.id}"] [data-folded]`)).toHaveAttribute('data-folded', 'false');
  expect(document.querySelector(`[data-editor-owner="${one.id}"] [data-folded]`)).toHaveAttribute('data-folded', 'true');
  expect(state().activeContent).toBe(two.id); expect(navigation).not.toHaveBeenCalled();
  fireEvent.click(button()); flushFrames();
  expect(scrolls).toEqual([{ owner: two.id, block: heading.id }, { owner: two.id, block: heading.id }]);
});

it('an inactive following owner is activated by its exact block link instead of scrolling a visible duplicate', () => {
  state().change((layout) => ({ ...layout, right: { ...layout.right, containers: [], active: null },
    main: { kind: 'split', id: newId('s'), direction: 'row', sizes: [50, 50], children: [makeGroup([one]), makeGroup([two, outline], two.id)] } }));
  state().focus(two.id); state().focus(outline.id); render(<Frame />);
  fireEvent.click(button()); flushFrames();
  expect(state().activeContent).toBe(two.id); expect(findTab(state().layout, two.id)?.group.active).toBe(two.id);
  expect(parsePageAddress(findTab(state().layout, two.id)!.tab.of!.href!)?.search.block).toBe(heading.id);
  expect(navigation).toHaveBeenCalledWith(two.id, expect.stringContaining(`/doc/${doc.id}?block=`), true);
  expect(scrolls).toEqual([]); expect(currentMark(one.id)).toBeNull();
});

it('does not fall through to a live duplicate when the selected snapshot does not contain the current heading', () => {
  two.of = { href: `/doc/${doc.id}?version=2#snapshot` };
  render(<Frame missingSecond />); fireEvent.click(button()); flushFrames();
  expect(scrolls).toEqual([]); expect(currentMark(one.id)).toBeNull(); expect(currentMark(two.id)).toBeNull();
  const address = contentAddress(findTab(state().layout, two.id)!.tab)!;
  expect(parsePageAddress(address)?.search).toMatchObject({ version: 2, block: heading.id });
  expect(parsePageAddress(address)?.hash).toBe('snapshot'); expect(state().activeContent).toBe(two.id);
  expect(navigation).toHaveBeenCalledWith(two.id, address, true);
});

it('fixed Outline A while B is focused reuses A’s exact version/hash and leaves B’s locator untouched', () => {
  one.of = { href: `/doc/${doc.id}?version=2#retained` };
  outline.scope = { mode: 'fixed', target: { kind: 'document', id: doc.id } };
  state().change((layout) => ({ ...layout, main: makeGroup([one, other], other.id) }));
  state().focus(other.id); render(<Frame />);
  fireEvent.click(button()); flushFrames();
  const address = contentAddress(findTab(state().layout, one.id)!.tab)!;
  expect(parsePageAddress(address)?.search).toMatchObject({ version: 2, block: heading.id });
  expect(parsePageAddress(address)?.hash).toBe('retained');
  expect(findTab(state().layout, other.id)?.tab.of?.href).toBe('/doc/another-document'); expect(state().activeContent).toBe(one.id);
  expect(navigation).toHaveBeenCalledWith(one.id, address, true);
  expect(findTab(state().layout, outline.id)?.tab.scope).toEqual(outline.scope);
});

it('fixed Outline opens A through the normal block address after A closes, preserving B and the closed entry', () => {
  outline.scope = { mode: 'fixed', target: { kind: 'document', id: doc.id } };
  state().change((layout) => ({ ...layout, main: makeGroup([one, other], other.id) }));
  state().focus(other.id); state().close({ kind: 'tab', tab: one.id }); render(<Frame />);
  fireEvent.click(button()); flushFrames();
  const pages = allTabs(state().layout).filter((tab) => contentTarget(tab)?.kind === 'document');
  const opened = pages.find((tab) => contentTarget(tab)?.id === doc.id);
  expect(opened).toBeDefined(); expect(opened?.id).not.toBe(one.id);
  expect(parsePageAddress(opened!.of!.href!)?.search.block).toBe(heading.id);
  expect(state().activeContent).toBe(opened!.id); expect(findTab(state().layout, other.id)?.tab).toBe(other);
  expect(state().closedTabs.some((entry) => entry.tab.id === one.id)).toBe(true); expect(scrolls).toEqual([]);
});

it('repeating a fixed block link reveals only its owner and stops if that owner closes before the frame', () => {
  one.of = { href: `/doc/${doc.id}?block=${heading.id}&version=2` };
  outline.scope = { mode: 'fixed', target: { kind: 'document', id: doc.id } };
  state().change((layout) => ({ ...layout, main: makeGroup([one, other], other.id) }));
  state().focus(other.id); render(<Frame />);
  fireEvent.click(button()); flushFrames();
  expect(scrolls).toEqual([{ owner: one.id, block: heading.id }]); expect(currentMark(one.id)).toBe(heading.id);
  scrolls.length = 0;
  fireEvent.click(button());
  act(() => { state().close({ kind: 'tab', tab: one.id }); }); flushFrames();
  expect(scrolls).toEqual([]); expect(state().activeContent).toBe(other.id);
});

it('does not revive a closed following owner or reveal content after a current refusal', () => {
  render(<Frame />); const stale = button();
  act(() => { state().close({ kind: 'tab', tab: two.id }); fireEvent.click(stale); }); flushFrames();
  expect(findTab(state().layout, two.id)).toBeNull(); expect(scrolls).toEqual([]); expect(navigation).not.toHaveBeenCalled();
  const current = button();
  act(() => { documentWriter.failed(doc.id, new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: 'Unavailable', detail: 'Unavailable', status: 404, request_id: 'outline-refusal' }, null)); fireEvent.click(current); });
  flushFrames(); expect(scrolls).toEqual([]); expect(navigation).not.toHaveBeenCalled();
});

it.each(['retired', 'hidden'] as const)('drops a queued highlight when its addressed editor becomes %s', (reason) => {
  render(<Frame />); fireEvent.click(button());
  act(() => {
    if (reason === 'retired') retireDocumentMarks();
    else document.querySelector<HTMLElement>(`[data-editor-owner="${two.id}"]`)?.setAttribute('hidden', '');
  });
  flushFrames();
  expect(scrolls).toEqual([]); expect(currentMark(two.id)).toBeNull(); expect(useMarks.getState().highlight).toBeNull();
});
