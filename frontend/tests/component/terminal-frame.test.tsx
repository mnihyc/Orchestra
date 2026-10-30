/** The shell's creation and scope help travel together; empty, refused and failed states have one message. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type { Door } from '@/api/workspace';
import type { EnvironmentTerminalCreateBody, EnvironmentTerminalCreateReply, TerminalSession } from '@/generated/api-types';
import type { EnvironmentWorkspaceAccess } from '@/features/files/door';
import { TerminalPanel } from '@/features/inspector/Terminal';
import { forgetViewActions, ViewActionSlot } from '@/features/workbench/actions';
import { makeTab } from '@/features/workbench/layout';
import { i18n, setLocale } from '@/i18n';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ terminal: true, open: vi.fn<(door: Door, body: EnvironmentTerminalCreateBody) => Promise<EnvironmentTerminalCreateReply>>() }));
vi.mock('@/api/terminals', () => ({
  shellsQuery: () => ({ queryKey: ['frame-test-shells'], queryFn: (): Promise<TerminalSession[]> => Promise.resolve([]) }),
  closedShellsQuery: () => ({ queryKey: ['frame-test-closed-shells'], queryFn: (): Promise<TerminalSession[]> => Promise.resolve([]) }),
  openShell: calls.open, closeShell: vi.fn(), atLimit: () => null, notAllowedReason: () => null,
}));
vi.mock('@/features/files/door', () => ({ useEnvironment: () => ({ environment: { terminal: calls.terminal, threads: [], threads_hidden: 0 } }) }));
vi.mock('@/features/inspector/TerminalScreen', () => ({ TerminalScreen: () => null }));

const person = me(1, 'ada', 'Ada');
const tab = makeTab('terminal');
const access: EnvironmentWorkspaceAccess = {
  door: { environmentId: 'environment', threadId: 'thread' }, thread: null,
  environment: { id: 'environment', name: 'Workspace', address: 'workspace', owner: userRef(person), placement: 'connected', ports_exposed: false, status: 'running' },
  directory: 'web', mayView: true, mayManage: true, mayWatchProcesses: true, mayShell: true,
};
function Frame({ moved = false }: { moved?: boolean }) {
  return <><header aria-label="Tab row">{!moved && <ViewActionSlot tab={tab} />}</header>
    <TerminalPanel access={access} tab={tab} />
    <aside aria-label="Section actions">{moved && <ViewActionSlot tab={tab} />}</aside></>;
}
afterEach(async () => { cleanup(); forgetViewActions('terminal'); calls.terminal = true; calls.open.mockReset(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) {
  it(`has one scope question and one create action after a move in ${locale}/${theme}`, async () => {
    const session = client.session.getState();
    try {
      await setLocale(locale);
      document.documentElement.setAttribute('data-theme', theme);
      client.session.setState({ userId: person.id, status: 'signed_in' });
      calls.open.mockRejectedValue(new Error('Opening refused'));
      const user = userEvent.setup();
      const view = show(<Frame />);
      const create = await screen.findByRole('button', { name: i18n.t('terminal.strip.new') });
      expect(within(screen.getByRole('banner', { name: 'Tab row' })).getByRole('button', { name: i18n.t('terminal.strip.new') })).toBe(create);
      expect(screen.getAllByRole('button', { name: i18n.t('terminal.strip.new') })).toHaveLength(1);
      expect(screen.queryByRole('note')).not.toBeInTheDocument();
      expect(screen.getByText(i18n.t('terminal.none'))).toBeVisible();
      expect(screen.queryByText(i18n.t('terminal.note.alone'))).not.toBeInTheDocument();
      const helpName = i18n.t('help.hint.label', { term: i18n.t('terminal.note.term') });
      await user.click(screen.getByRole('button', { name: helpName }));
      expect(await screen.findByText(i18n.t('terminal.note.alone'))).toBeVisible();
      await user.keyboard('{Escape}');
      expect(screen.queryByRole('note')).not.toBeInTheDocument();
      view.rerender(<Frame moved />);
      expect(within(screen.getByRole('complementary', { name: 'Section actions' })).getByRole('button', { name: i18n.t('terminal.strip.new') })).toBe(create);
      expect(screen.getAllByRole('button', { name: i18n.t('terminal.strip.new') })).toHaveLength(1);
      expect(screen.queryByRole('note')).not.toBeInTheDocument();
      await user.click(create);
      expect(calls.open).toHaveBeenCalledExactlyOnceWith(access.door, { cols: 80, rows: 24, cwd: 'web' });
      expect(await screen.findByRole('alert')).toBeVisible();
      expect(screen.queryByText(i18n.t('terminal.none'))).not.toBeInTheDocument();
      expect(screen.queryByRole('note')).not.toBeInTheDocument();
      expect(screen.getAllByRole('button', { name: helpName })).toHaveLength(1);
    } finally { client.session.setState(session); }
  });
  it(`shows only the native shell refusal when shells are switched off in ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme; calls.terminal = false;
    show(<Frame />);
    expect(await screen.findByText(i18n.t('terminal.refused.title'))).toBeVisible();
    expect(screen.queryByText(i18n.t('terminal.none'))).not.toBeInTheDocument();
    expect(screen.queryByRole('note')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('terminal.strip.new') })).toBeDisabled();
    expect(calls.open).not.toHaveBeenCalled();
  });
}
