/** A retained tab keeps its identity and unsaved protection while React pauses its hidden body. */
import { Activity, useState } from 'react';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MessageSquareText } from 'lucide-react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { useUnsavedMark } from '@/features/files/Editing';
import { allTabs, makeGroup, makeTab } from '@/features/workbench/layout';
import { loadWorkspace } from '@/features/workbench/persistence';
import { useTabName, useUnsavedGuard, useWorkbench } from '@/features/workbench/store';

const tab = makeTab('file', { path: 'notes.txt', environment: 'workspace' });
function Body() {
  const [draft, setDraft] = useState('');
  useTabName(tab.id, 'Notes', MessageSquareText);
  useUnsavedMark(tab.id, draft !== '');
  return <input aria-label="Draft" value={draft} onChange={(event) => setDraft(event.target.value)} />;
}
function Frame({ shown }: { shown: boolean }) {
  useUnsavedGuard();
  return <Activity mode={shown ? 'visible' : 'hidden'}><Body /></Activity>;
}
function wouldWarn() {
  const event = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(event);
  return event.defaultPrevented;
}
beforeEach(() => {
  useWorkbench.getState().leave();
  useWorkbench.setState({ layout: { ...useWorkbench.getState().layout, main: makeGroup([tab]) } });
});
afterEach(() => { cleanup(); useWorkbench.getState().leave(); });

it('retains the tab title, icon, draft and unload warning while another tab is in front', async () => {
  const view = render(<Frame shown />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), { target: { value: 'Keep these edits' } });
  expect(wouldWarn()).toBe(true);
  view.rerender(<Frame shown={false} />);
  await act(async () => { await Promise.resolve(); });
  expect(useWorkbench.getState().named.get(tab.id)).toEqual({ title: 'Notes', icon: MessageSquareText, source: 'view' });
  expect(useWorkbench.getState().unsaved.has(tab.id)).toBe(true);
  expect(wouldWarn()).toBe(true);
  view.rerender(<Frame shown />);
  expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('Keep these edits');
});

it('retires paused metadata and its warning when the tab actually leaves the layout', () => {
  const view = render(<Frame shown />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), { target: { value: 'A draft' } });
  view.rerender(<Frame shown={false} />);
  act(() => { useWorkbench.getState().change((layout) => ({ ...layout, main: makeGroup() })); });
  expect(useWorkbench.getState().named.has(tab.id)).toBe(false);
  expect(useWorkbench.getState().unsaved.has(tab.id)).toBe(false);
  expect(wouldWarn()).toBe(false);
});

it('restores the default layout while keeping an unsaved file and its identity', () => {
  const view = render(<Frame shown />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), { target: { value: 'A draft' } });
  view.rerender(<Frame shown={false} />);
  act(() => { useWorkbench.getState().reset(); });
  expect(allTabs(useWorkbench.getState().layout)).toContainEqual(tab);
  expect(useWorkbench.getState().named.get(tab.id)?.title).toBe('Notes');
  expect(useWorkbench.getState().unsaved.has(tab.id)).toBe(true);
  expect(wouldWarn()).toBe(true);
});

it('keeps the unsaved file entrance across an immediate reset and reload', () => {
  useWorkbench.getState().enter('layout-recovery-reader', 'thread');
  useWorkbench.getState().change((layout) => ({ ...layout, main: makeGroup([makeTab('screen'), tab]) }));
  const view = render(<Frame shown />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), { target: { value: 'Keep after reset' } });
  act(() => { useWorkbench.getState().reset(); });
  const saved = loadWorkspace('layout-recovery-reader', 'thread').kept;
  expect(saved).not.toBeNull();
  expect(allTabs(saved!.layout)).toContainEqual(tab);
  view.unmount();
  useWorkbench.getState().leave();
  useWorkbench.getState().enter('layout-recovery-reader', 'thread');
  expect(allTabs(useWorkbench.getState().layout)).toContainEqual(tab);
});
