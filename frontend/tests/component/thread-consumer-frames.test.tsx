import { defaultThreadRead } from '@/realtime/threadScope';
/** A bad cached conversation frame is contained in its own view; a complete native frame restores it without reloading the application. */
import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { ThreadScreen } from '@/features/thread/Thread';
import { isThreadForScreen, threadCapabilities } from '@/features/thread/cached';
import { mayExportThread } from '@/features/export/pending';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { agentMessage } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ reload: vi.fn() }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: () => undefined, useOrgTreeFeed: () => undefined, reloadThread: calls.reload }));
vi.mock('@/shell/screen', async (load) => ({ ...await load<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/features/thread/Header', () => ({ ThreadHeader: ({ thread }: { thread: Thread }) => <header><h1>{thread.name}</h1></header> }));
vi.mock('@/features/composer/Composer', () => ({ Composer: () => <textarea aria-label="Thread draft" /> }));
vi.mock('@/features/composer/QueueTray', () => ({ QueueTray: () => null }));
vi.mock('@/features/thread/ConnectionBanner', () => ({ ConnectionBanner: () => null }));
vi.mock('@/features/thread/BlockedBanner', () => ({ BlockedBanner: () => null }));
vi.mock('@/features/thread/LockBanner', () => ({ LockBanner: () => null }));
vi.mock('@/features/source/Preparation', () => ({ SourcePreparation: () => null }));
vi.mock('@/features/source/Review', () => ({ ReviewTray: () => null }));
vi.mock('@/features/timeline/FindInConversation', () => ({ FindInConversation: () => null }));
vi.mock('@/features/timeline/Timeline', () => ({ Timeline: () => <p>The redirect has been checked.</p> }));
vi.mock('@/features/ports/LocalAddresses', () => ({ LocalAddresses: ({ children }: { children: ReactNode }) => children }));

let native: Thread;
let theme: string | null = null;
beforeEach(() => {
  calls.reload.mockReset(); theme = document.documentElement.getAttribute('data-theme');
  const world = new World(); const person = world.people.get('ada');
  const node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('A native thread is required');
  native = { ...world.threads.thread(person, node), state: 'idle' };
  threadWriter.reset(native.id, { thread: native, items: [{ ...agentMessage('answer', 'The redirect has been checked.'), thread_id: native.id, seq_created: 1, seq_updated: 1 }] }, false);
});
afterEach(async () => { threadWriter.evict(native.id); await setLocale('en'); if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); });

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('retains the conversation without capability guesses, and contains an incomplete frame with Retry in %s/%s', async (locale, look) => {
  await setLocale(locale); document.documentElement.setAttribute('data-theme', look);
  const missingCan = { ...native };
  Reflect.deleteProperty(missingCan, 'can');
  expect(isThreadForScreen(missingCan, native.id)).toBe(true);
  expect(threadCapabilities(missingCan)).toBeNull(); expect(mayExportThread(missingCan)).toBeNull();
  act(() => threadWriter.apply(native.id, { thread: { ...missingCan, seq: native.seq + 1 } }));
  show(<><input aria-label="Another view's draft" defaultValue="Keep these words" /><ThreadScreen threadId={native.id} item={null} /></>);
  expect(screen.getByRole('heading', { name: native.name })).toBeVisible();
  expect(screen.getByText('The redirect has been checked.')).toBeVisible();
  const thin = { ...native, seq: native.seq + 2 };
  Reflect.deleteProperty(thin, 'input_access');
  act(() => threadWriter.apply(native.id, { thread: thin }));
  expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('thread.screen.incomplete'));
  expect(screen.getByRole('textbox', { name: "Another view's draft" })).toHaveValue('Keep these words');
  expect(threadViewOf(native.id).items.has('answer')).toBe(true);
  expect(calls.reload).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(calls.reload).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id));
  act(() => threadWriter.apply(native.id, { thread: { ...native, name: 'The redirect is fixed', seq: native.seq + 3 } }));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'The redirect is fixed' })).toBeVisible();
  expect(screen.getByText('The redirect has been checked.')).toBeVisible();
  expect(screen.getByRole('textbox', { name: "Another view's draft" })).toHaveValue('Keep these words');
  expect(calls.reload).toHaveBeenCalledTimes(1);
});

it('refuses foreign, incomplete or wrong-thread frames and keeps unknown capabilities distinct from denied or granted', () => {
  for (const value of [null, [], {}, { id: native.id, kind: 'thread', name: native.name }, { ...native, id: 'another' }, { ...native, kind: 'topic' }, { ...native, binding: {} }, { ...native, owner: null }]) {
    expect(isThreadForScreen(value, native.id)).toBe(false);
  }
  expect(threadCapabilities({ can: { export: 'true', fork: null, archive: false, place: true } })).toEqual({ archive: false, place: true });
  expect(threadCapabilities({ ...native, can: undefined })).toBeNull();
  expect(mayExportThread({ ...native, can: { ...native.can, export: false } })).toBe(false);
  expect(mayExportThread({ ...native, can: { ...native.can, export: true } })).toBe(true);
});
