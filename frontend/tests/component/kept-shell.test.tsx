/** With a shell kept and a network that never answers, the shell, its tabs and its tree are on the screen; after sign-out nothing of it is left. */
import { act, render, screen } from '@testing-library/react';
import { createStore } from 'zustand/vanilla';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import { KeptShellCover } from '@/shell/KeptShellCover';
import { forgetWhenSignedOut, KEPT_SHELL_DAYS, readKeptShell, uncover, useCover, writeKeptShell } from '@/shell/keptShell';

const kept = () => ({
  v: 1 as const, user: 'reader', name: 'Ada Lovelace', kept: Date.now(),
  left: { open: true, size: 280 }, right: { open: false, size: 320 },
  tabs: [{ id: 'one', title: 'Fix the build', kind: 'thread', front: true }, { id: 'two', title: 'Release notes', kind: 'document', front: false }],
  tree: { org: 'acme', rows: [{ depth: 0, kind: 'project', name: 'Website' }, { depth: 1, kind: 'topic', name: 'Launch' }] },
});

const sessionBefore = client.session.getState();
beforeEach(() => { client.session.setState({ status: 'signed_in', userId: 'reader' }); vi.useFakeTimers(); useCover.setState({ covering: true }); });
afterEach(() => { client.session.setState(sessionBefore); vi.useRealTimers(); localStorage.clear(); useCover.setState({ covering: true }); });

it('draws the confirmed reader’s tabs and tree while the shell’s own answer is pending', () => {
  writeKeptShell(kept());
  render(<KeptShellCover />);
  const cover = document.querySelector('[data-kept-shell]');
  expect(cover).not.toBeNull();
  expect(cover).toHaveTextContent('Fix the build');
  expect(cover).toHaveTextContent('Release notes');
  expect(cover).toHaveTextContent('Website');
  expect(cover).toHaveTextContent('Launch');
  expect(cover).toHaveTextContent('A');
  // It is a picture of the shell: nothing in it can be reached or read out.
  expect(cover).toHaveAttribute('aria-hidden', 'true');
  expect(cover).toHaveAttribute('inert');
  // A network that never answers leaves it where it is, however long that is.
  act(() => { vi.advanceTimersByTime(10 * 60_000); });
  expect(document.querySelector('[data-kept-shell]')).not.toBeNull();
  // The frame standing admitted takes its place.
  act(() => { uncover(); });
  expect(document.querySelector('[data-kept-shell]')).toBeNull();
});

it('draws nothing where nothing was kept', () => {
  render(<KeptShellCover />);
  expect(document.querySelector('[data-kept-shell]')).toBeNull();
});

it('leaves nothing of the shell when the session says nobody is signed in', () => {
  writeKeptShell(kept());
  const session = createStore<{ status: string }>(() => ({ status: 'unknown' }));
  const stop = forgetWhenSignedOut(session);
  render(<KeptShellCover />);
  expect(screen.queryByText('Fix the build')).toBeInTheDocument();
  act(() => { session.setState({ status: 'signed_out' }); });
  expect(document.querySelector('[data-kept-shell]')).toBeNull();
  expect(localStorage.getItem('orch.reader.shell')).toBeNull();
  expect(readKeptShell('reader')).toBeNull();
  stop();
});

it('does not believe a record older than it keeps one', () => {
  writeKeptShell({ ...kept(), kept: Date.now() - (KEPT_SHELL_DAYS + 1) * 86_400_000 });
  render(<KeptShellCover />);
  expect(document.querySelector('[data-kept-shell]')?.textContent ?? '').not.toContain('Fix the build');
});
