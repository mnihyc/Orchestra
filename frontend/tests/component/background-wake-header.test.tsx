import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { BackgroundWorkLine, NativeBackgroundCommands } from '@/features/running/BackgroundWork';
import type { BackgroundCommand } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { backgroundRoster, backgroundWake, backgroundWork } from '../doubles/background-wake';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`names known work and opens the setting with compact controls at ${width}px ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const setting = vi.fn(), end = vi.fn();
    const task = backgroundWork({ label: 'Build web with a long name that remains readable on a phone' });
    show(<div style={{ width }}><BackgroundWorkLine status={backgroundWake({ tasks: [task, backgroundWork({ id: 'review', kind: 'subagent', label: 'Review auth', can_end: false })] })} canManage mayEnd onSetting={setting} onEnd={end} /></div>);
    const work = screen.getByRole('group', { name: i18n.t('backgroundWake.work') });
    expect(within(work).getByText(task.label)).toBeVisible();
    expect(within(work).getByText('Review auth')).toBeVisible();
    expect(within(work).getByText(i18n.t('backgroundWake.cannotEnd'))).toBeVisible();
    expect(within(work).getByText(i18n.t('backgroundWake.willWake'))).toBeVisible();
    await userEvent.click(within(work).getByRole('button', { name: i18n.t('backgroundWake.settingOn') }));
    expect(setting).toHaveBeenCalledOnce();
    await userEvent.click(within(work).getByRole('button', { name: i18n.t('backgroundWake.endNamed', { name: task.label }) }));
    expect(end).toHaveBeenCalledExactlyOnceWith(task);
    expect(work.querySelector('p')).toBeNull();
  });
}

it.each(['running', 'completed', 'failed', 'stopped', 'lost'] as const)('shows the recorded %s state and offers End only for running work', (state) => {
  show(<BackgroundWorkLine status={backgroundWake({ tasks: [backgroundWork({ state })] })} canManage mayEnd onSetting={vi.fn()} onEnd={vi.fn()} />);
  expect(screen.getAllByText(i18n.t(`backgroundWake.states.${state}`)).length).toBeGreaterThan(0);
  expect(screen.queryByRole('button', { name: 'End Build web' }) !== null).toBe(state === 'running');
  if (state === 'lost') { expect(screen.getByText(i18n.t('backgroundWake.lost'))).toBeVisible(); expect(screen.queryByText(i18n.t('backgroundWake.willWake'))).not.toBeInTheDocument(); }
});

it('shows effective and pending values without offering management or End to a viewer', () => {
  show(<BackgroundWorkLine status={backgroundWake({ setting: false, effective: true, refresh_deferred: true, held_completions: 2 })} canManage={false} mayEnd={false} onSetting={vi.fn()} onEnd={vi.fn()} />);
  expect(screen.getByText(i18n.t('backgroundWake.nextInput'))).toBeVisible();
  expect(screen.getByText(i18n.t('backgroundWake.settingOn'))).toBeVisible();
  expect(screen.getByText(i18n.t('backgroundWake.pendingOff'))).toBeVisible();
  expect(screen.getByText(i18n.t('backgroundWake.held', { count: 2 }))).toBeVisible();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('does not turn an absent observation into an empty list or a wake promise', () => {
  const { container } = show(<BackgroundWorkLine status={null} canManage mayEnd onSetting={vi.fn()} onEnd={vi.fn()} />);
  expect(container).toBeEmptyDOMElement();
});

it.each(['en', 'zh-CN'] as const)('keeps the Off notice honest until a pending On value applies (%s)', async (locale) => {
  await setLocale(locale);
  show(<BackgroundWorkLine status={backgroundWake({ setting: true, effective: false })} canManage mayEnd onSetting={vi.fn()} onEnd={vi.fn()} />);
  expect(screen.getByText(i18n.t('backgroundWake.nextInput'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('backgroundWake.settingOff') })).toBeVisible();
  expect(screen.getByText(i18n.t('backgroundWake.pendingOn'))).toBeVisible();
  expect(screen.queryByText(i18n.t('backgroundWake.willWake'))).not.toBeInTheDocument();
});

it('uses safe generic labels for masked task names without printing native identities', () => {
  const { container } = show(<BackgroundWorkLine status={backgroundWake({ tasks: [backgroundWork({ id: 'opaque-record', label: '' })] })} canManage mayEnd onSetting={vi.fn()} onEnd={vi.fn()} />);
  expect(screen.getByText(i18n.t('backgroundWake.unnamedCommand'))).toBeVisible();
  expect(container).not.toHaveTextContent('opaque-record');
});

it.each(['ambiguous source', 'lost source', 'exact run'] as const)('keeps a later native run visible beside a recorded %s', (association) => {
  const task = backgroundWork({ state: association === 'lost source' ? 'lost' : 'running', end_id: association === 'exact run' ? 'first-run' : null });
  const command: BackgroundCommand = { id: 'first-run', segment_id: 'origin-segment', item_id: task.item_id, turn_id: task.turn_id,
    command: 'First run', command_truncated: false, cwd: null, cwd_redacted: false, place: 'environment', since: null, cpu_percent: null, memory_bytes: null };
  const later = { ...command, id: 'later-run', command: 'Later run' };
  const items = association === 'lost source' ? [later] : [command, later];
  show(<NativeBackgroundCommands roster={backgroundRoster({ items })} represented={[task]} mayEnd onEnd={vi.fn()} />);
  expect(screen.getByText('Later run')).toBeVisible();
  expect(screen.queryByText('First run') !== null).toBe(association === 'ambiguous source');
});
