/** The environment consumes its native readiness report and uses only the allowed recovery acts. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { EnvironmentReadinessNotice } from '@/features/resources/SandboxReadiness';
import { openChange } from '@/features/change/target';
import { i18n, setLocale } from '@/i18n';
import { RUNS_ON } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { sandboxReadiness } from '../fixtures/sandbox-readiness';
import { show } from './support';

vi.mock('@/features/change/target', () => ({ openChange: vi.fn() }));
afterEach(async () => { cleanup(); vi.clearAllMocks(); delete document.documentElement.dataset.theme; await setLocale('en'); });
function nativeEnvironment() {
  const world = new World(), ada = world.people.get('ada');
  if (ada === undefined) throw new Error('The native reader is required');
  const result = world.resources.inventoryView(ada, 'environment', RUNS_ON.workspace);
  if (result?.kind !== 'environment') throw new Error('The full native environment is required');
  return result.value;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows one compact native refusal and the existing manager recovery plan ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const native = nativeEnvironment();
    const report = sandboxReadiness(native, native.machine);
    show(<EnvironmentReadinessNotice environment={{ ...native, sandbox_readiness: report }} />);
    const title = i18n.t('sandboxReadiness.blocked', { name: native.name });
    expect(screen.getByRole('alert')).toHaveTextContent(title);
    expect(screen.queryByRole('link', { name: i18n.t('sandboxReadiness.openEnvironment') })).not.toBeInTheDocument();
    const comparison = i18n.t('sandboxReadiness.limit', { requested: i18n.t('sandboxReadiness.modes.open.name'), widest: i18n.t('sandboxReadiness.modes.workspace.name'), source: i18n.t('sandboxReadiness.sources.machine_policy.name') });
    expect(screen.getByRole('alert')).not.toHaveTextContent(comparison);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: title }) }));
    expect(await screen.findByRole('dialog', { name: title })).toHaveTextContent(comparison);
    await userEvent.keyboard('{Escape}');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('change.env.menu.move') }));
    expect(openChange).toHaveBeenCalledOnce();
    expect(openChange).toHaveBeenCalledWith({ kind: 'environment', id: native.id, name: native.name }, { kind: 'move' });
  });

  it(`keeps ready or absent reports quiet and does not give a viewer management ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const native = nativeEnvironment(), report = sandboxReadiness(native, native.machine);
    const view = show(<EnvironmentReadinessNotice environment={{ ...native, sandbox_readiness: null }} />);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    view.rerender(<EnvironmentReadinessNotice environment={{ ...native, sandbox_readiness: { ...report, sandbox: { ...report.sandbox, ready: true, widest: 'open', machine_limit: 'open' } } }} />);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    view.rerender(<EnvironmentReadinessNotice environment={{ ...native, access: { ...native.access, permissions: native.access.permissions.filter((permission) => permission !== 'environment.manage') }, sandbox_readiness: { ...report, policy_manage: false } }} />);
    expect(screen.getByRole('alert')).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('change.env.menu.move') })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: i18n.t('sandboxReadiness.openPolicy', { name: report.machine?.name ?? '' }) })).not.toBeInTheDocument();
    expect(openChange).not.toHaveBeenCalled();
  });
}
