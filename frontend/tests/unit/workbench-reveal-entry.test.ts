/** An early visible act survives workspace entry without crossing reader authority. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { MessageCircleQuestionMark } from 'lucide-react';
import { client } from '@/api/client';
import { allTabs } from '@/features/workbench/layout';
import { holdViewReveals, revealView } from '@/features/workbench/openView';
import { useWorkbench } from '@/features/workbench/store';
import { registerView } from '@/features/workbench/views';
import { readViewChoice } from '@/lib/viewChoices';

const READER = 'early-view-reader';
const VIEW = 'entry-check';
let unregister: () => void = () => undefined;
const sessionBefore = client.session.getState();
const state = () => useWorkbench.getState();
beforeEach(() => {
  state().leave(); localStorage.clear();
  unregister = registerView({ name: VIEW, title: 'ask.title', about: 'ask.about', icon: MessageCircleQuestionMark, screens: 'all', area: 'right', container: VIEW, singleton: true, window: true, body: () => null });
  client.session.setState({ status: 'signed_in', userId: READER, readerVersion: 1 });
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 0));
});
afterEach(async () => {
  state().leave(); unregister(); client.session.setState(sessionBefore, true);
  await Promise.resolve(); localStorage.clear(); vi.unstubAllGlobals();
});

it('keeps an early requested view after the initial workspace is admitted', async () => {
  revealView(VIEW, {});
  state().enter(READER, 'topic');
  await Promise.resolve();
  expect(allTabs(state().layout)).toEqual(expect.arrayContaining([expect.objectContaining({ view: VIEW, of: {} })]));
  expect(state().screen).toBe('topic');
  expect(state().overlay).toBe('right');
  expect(readViewChoice(READER, 'global', 'mobileView', '')).toBe(VIEW);
});

it('drops an early requested target when the person changes before entry', async () => {
  revealView(VIEW, { conversation: 'private-conversation' });
  client.session.setState({ userId: 'next-view-reader', readerVersion: 2 });
  state().enter('next-view-reader', 'topic');
  await Promise.resolve();
  expect(allTabs(state().layout).some((tab) => tab.view === VIEW)).toBe(false);
});

it('drops an early requested target when the same person changes reader authority', async () => {
  revealView(VIEW, { conversation: 'private-conversation' });
  client.session.setState({ readerVersion: 2 }); state().enter(READER, 'topic');
  await Promise.resolve();
  expect(allTabs(state().layout).some((tab) => tab.view === VIEW)).toBe(false);
});

it('still reveals immediately in an already admitted workspace', () => {
  state().enter(READER, 'topic'); revealView(VIEW, {});
  expect(allTabs(state().layout).some((tab) => tab.view === VIEW)).toBe(true);
  expect(state().overlay).toBe('right');
});

it('drops a target when authority changes after entry but before its reveal', async () => {
  revealView(VIEW, { conversation: 'private-conversation' });
  state().enter(READER, 'topic'); client.session.setState({ readerVersion: 2 });
  await Promise.resolve();
  expect(allTabs(state().layout).some((tab) => tab.view === VIEW)).toBe(false);
});

it('waits for the frame hosts after restoring the workspace', async () => {
  const admission = holdViewReveals(READER, 1);
  try {
    revealView(VIEW, {}); state().enter(READER, 'topic');
    await Promise.resolve();
    expect(allTabs(state().layout).some((tab) => tab.view === VIEW)).toBe(false);
    admission.admit(); await Promise.resolve();
    expect(allTabs(state().layout).some((tab) => tab.view === VIEW)).toBe(true);
    expect(state().overlay).toBe('right');
  } finally { admission.release(); }
});

it('drops a waiting reveal when its frame is removed', async () => {
  const admission = holdViewReveals(READER, 1);
  revealView(VIEW, { conversation: 'private-conversation' });
  state().enter(READER, 'topic'); admission.release();
  await Promise.resolve();
  expect(allTabs(state().layout).some((tab) => tab.view === VIEW)).toBe(false);
});
