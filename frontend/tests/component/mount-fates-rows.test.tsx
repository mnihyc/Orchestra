/** One compact row per mount a plan may change: preselected to its default, the copy's size, and Replace or Keep both where the target holds data. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { withFate, withPick } from '@/features/change/choices';
import { MountFates } from '@/features/change/MountFates';
import type { PlanMount } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const base = { action: 'keep', path: '/mnt/x', mode: 'read_only', detached_because: null, copy_unavailable: null, copy: null, data_kind: null, fingerprints_available: false, fingerprints: null } as const;

const notes: PlanMount = { ...base, mount: { id: 'notes', name: 'notes', address: 'acme.notes' }, fate: 'as_is', default_fate: 'as_is', fates: ['detach', 'as_is', 'copy'] };
const data: PlanMount = {
  ...base, mount: { id: 'data', name: 'data', address: 'acme.data' }, fate: 'copy', default_fate: 'copy', fates: ['detach', 'as_is', 'copy'],
  copy: { measured: true, file_count: 3, total_bytes: 8192, location: { made: false, kind: 'volume', source: null, storage: null }, holds: { measured: true, file_count: 2, total_bytes: 2048 }, into: null, replace: true },
};
const fixed: PlanMount = { ...base, mount: { id: 'fixed', name: 'fixed', address: 'acme.fixed' }, fate: 'as_is', default_fate: 'as_is', fates: ['as_is'], copy_unavailable: 'unchanged' };
const here: PlanMount = { ...notes, mount: { id: 'here', name: 'here', address: 'acme.here' }, fates: ['detach', 'as_is'], copy_unavailable: 'same_machine' };

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) describe(locale, () => {
  it('checks only when pressed and states the result and server time in one quiet line', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const checkedAt = '2026-10-25T14:03:00.000Z';
    const onCheck = vi.fn();
    const synced: PlanMount = { ...fixed, fingerprints_available: true };
    const { rerender } = show(<MountFates mounts={[synced, notes]} picks={{}} onPick={vi.fn()} onCheck={onCheck} />);
    expect(onCheck).not.toHaveBeenCalled();
    expect(screen.getAllByRole('button', { name: t('mountPlan.fingerprints.check') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: t('mountPlan.fingerprints.check') }));
    expect(onCheck).toHaveBeenCalledTimes(1);
    rerender(<MountFates mounts={[synced]} picks={{}} onPick={vi.fn()} onCheck={onCheck} checking />);
    const checking = screen.getByRole('button', { name: t('mountPlan.fingerprints.checking') });
    expect(checking).toHaveAttribute('aria-busy', 'true');
    await userEvent.click(checking);
    expect(onCheck).toHaveBeenCalledTimes(1);
    for (const matches of [true, false]) {
      rerender(<MountFates mounts={[{ ...synced, fingerprints: { matches, checked_at: checkedAt } }]} picks={{}} onPick={vi.fn()} onCheck={onCheck} />);
      const result = screen.getByRole('status');
      expect(result).toHaveTextContent(`${t(matches ? 'mountPlan.fingerprints.same' : 'mountPlan.fingerprints.differs')} · `);
      expect(result.querySelector('time')).toHaveAttribute('datetime', checkedAt);
      expect(result).toHaveClass('text-sm', 'text-text-muted');
    }
    const failure = new ApiError({ type: 'orch:error:plan.blocked', code: 'plan.blocked', title: 'Blocked', status: 409, detail: 'The check failed', request_id: 'check', blockers: [{ code: 'mount_fingerprint_unavailable', message: 'The host is offline' }] }, null);
    rerender(<MountFates mounts={[synced]} picks={{}} onPick={vi.fn()} onCheck={onCheck} checkError={failure} />);
    expect(screen.getByRole('alert')).toHaveTextContent(t('mountPlan.fingerprints.failed'));
    expect(screen.getByRole('alert')).toHaveTextContent('The host is offline');
  });

  it('offers each changeable mount its three fates, preselected, and sends only what the person changes', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    const onPick = vi.fn();
    show(<MountFates mounts={[notes, data, fixed, here]} picks={{}} onPick={onPick} />);
    const rows = screen.getByRole('table', { name: t('mountPlan.fates.label') });
    expect(within(rows).getAllByRole('row').slice(1)).toHaveLength(3);
    expect(within(rows).queryByText('fixed')).not.toBeInTheDocument();
    const fates = screen.getByRole('combobox', { name: t('mountPlan.fates.of', { mount: 'notes' }) });
    expect(fates).toHaveTextContent(t('choiceCatalog.mountFate.as_is.name'));
    await user.click(fates);
    const offered = screen.getByRole('listbox');
    expect(within(offered).getAllByRole('option')).toHaveLength(3);
    expect(within(offered).getAllByRole('option').map((option) => option.textContent)).toEqual((['detach', 'as_is', 'copy'] as const).map((fate) => t(`choiceCatalog.mountFate.${fate}.name`)));
    for (const fate of ['detach', 'as_is', 'copy'] as const) expect(within(offered).getByRole('option', { name: t(`choiceCatalog.mountFate.${fate}.name`) })).toBeVisible();
    await user.click(within(offered).getByRole('option', { name: t('choiceCatalog.mountFate.copy.name') }));
    expect(onPick).toHaveBeenLastCalledWith('notes', { fate: 'copy', into: null });
    const sameMachine = screen.getByRole('combobox', { name: t('mountPlan.fates.of', { mount: 'here' }) });
    await user.click(sameMachine);
    expect(screen.getByRole('option', { name: t('choiceCatalog.mountFate.copy.name') })).toHaveAttribute('aria-disabled', 'true');
  });

  it('shows what a copy carries and asks whether it replaces what is there or keeps both', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    const onPick = vi.fn();
    show(<MountFates mounts={[data]} picks={{}} onPick={onPick} />);
    expect(screen.getAllByText(t('liveWork.files', { count: 3, value: '3' }) + ' · 8 KiB')[0]).toBeVisible();
    const into = screen.getByRole('combobox', { name: t('mountPlan.fates.into', { mount: 'data' }) });
    await user.click(into);
    await user.click(screen.getByRole('option', { name: t('mountPlan.fates.keepBoth') }));
    expect(onPick).toHaveBeenLastCalledWith('data', { fate: 'copy', into: 'new_location' });
    await user.click(screen.getByRole('combobox', { name: t('mountPlan.fates.of', { mount: 'data' }) }));
    await user.click(screen.getByRole('option', { name: t('choiceCatalog.mountFate.detach.name') }));
    expect(onPick).toHaveBeenLastCalledWith('data', { fate: 'detach', into: null });
  });

  it('offers a shared choice while preserving each row override and impossible choices', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const onPickAll = vi.fn();
    const onPick = vi.fn();
    const rendered = show(<MountFates mounts={[notes, data]} picks={{}} onPick={onPick} onPickAll={onPickAll} />);
    const shared = screen.getByRole('combobox', { name: t('liveWork.allMounts') });
    expect(shared).toHaveTextContent(t('liveWork.perMount'));
    await userEvent.click(shared);
    await userEvent.click(screen.getByRole('option', { name: t('choiceCatalog.mountFate.copy.name') }));
    expect(onPickAll).toHaveBeenLastCalledWith('copy');
    const picks = withFate({}, [notes, data], 'copy');
    expect(picks).toEqual({ notes: { fate: 'copy', into: null } });
    const overridden = withPick(picks, 'data', { fate: 'copy', into: 'new_location' });
    expect(overridden.data).toEqual({ fate: 'copy', into: 'new_location' });
    expect(withFate(overridden, [notes, data], null)).toEqual({});
    rendered.rerender(<MountFates mounts={[notes, here]} picks={{}} onPick={onPick} onPickAll={onPickAll} />);
    await userEvent.click(screen.getByRole('combobox', { name: t('liveWork.allMounts') }));
    expect(screen.getByRole('option', { name: t('choiceCatalog.mountFate.copy.name') })).toHaveAttribute('aria-disabled', 'true');
  });

  it('gives a mount back to its default when that is chosen again', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    const onPick = vi.fn();
    show(<MountFates mounts={[notes]} picks={{ notes: { fate: 'detach', into: null } }} onPick={onPick} />);
    const fates = screen.getByRole('combobox', { name: t('mountPlan.fates.of', { mount: 'notes' }) });
    expect(fates).toHaveTextContent(t('choiceCatalog.mountFate.detach.name'));
    await user.click(fates);
    await user.click(screen.getByRole('option', { name: t('choiceCatalog.mountFate.as_is.name') }));
    expect(onPick).toHaveBeenLastCalledWith('notes', null);
  });
});
