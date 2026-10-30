/** UA-12: only a root's owners see missing setup, once, with compact actions in either language/theme. */
import { cleanup, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as FirstStepsApi from '@/api/firstSteps';
import type * as Auth from '@/api/auth';
import type * as Nodes from '@/api/nodes';
import type * as Personal from '@/api/personal';
import { Checklist } from '@/features/onboarding/Checklist';
import type { FirstSteps, FirstStepsChoice, Org } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ACME, me, memberAccess, org, personalHomeOf, userRef } from '../doubles/fixtures';
import { show } from './support';

const observed = vi.hoisted(() => ({ data: undefined as FirstSteps | undefined, reads: vi.fn(), requests: vi.fn(), save: vi.fn<(choice: FirstStepsChoice) => Promise<Record<string, unknown>>>() }));
const person = me(1, 'ada', 'Ada');
const owner = userRef(person);
const root = org(ACME.org, 'setup-team', 'Setup team', owner, {
  access: { ...memberAccess, owns: true, permissions: ['org.view', 'org.manage', 'settings.manage', 'members.manage'] },
});
const solo = org(personalHomeOf(person), 'ada', 'Ada', owner, { org_kind: 'personal', access: root.access });
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');

vi.mock('@/api/firstSteps', async (original) => ({ ...await original<typeof FirstStepsApi>(),
  firstStepsQuery: (id: string | null) => {
    observed.reads(id);
    return { queryKey: ['ua12-first-steps', id], staleTime: Infinity, initialData: observed.data,
      queryFn: () => { observed.requests(); return observed.data === undefined ? Promise.reject(new Error('setup was not observed')) : Promise.resolve(observed.data); } };
  },
  setFirstSteps: observed.save,
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(),
  meQuery: { queryKey: ['ua12-me'], staleTime: Infinity, queryFn: () => Promise.resolve(person), initialData: () => person },
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(),
  orgsQuery: { queryKey: ['ua12-orgs'], staleTime: Infinity, queryFn: () => Promise.resolve([solo, root]), initialData: () => [solo, root] },
}));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], staleTime: Infinity, queryFn: () => Promise.resolve({}), initialData: {} },
}));

function facts(place: Org = root): FirstSteps {
  const source = { id: place.id, kind: 'org' as const, name: place.name, redacted: false };
  return { org: source, work_mode: 'team', invitation_roles: [], start_here: null, as_of: '2026-10-15T12:00:00Z', steps: [
    { id: 'team', state: 'done', fact: 'team_chosen', scripted: false, source, action: null },
    { id: 'provider', state: 'open', fact: 'not_done', scripted: false, source: null, action: { url: '/resources/providers', allowed: true, reason: null } },
    { id: 'models', state: 'open', fact: 'not_done', scripted: false, source: null, action: { url: '/resources/providers?tab=models', allowed: true, reason: null } },
    { id: 'work', state: 'open', fact: 'not_done', scripted: false, source: null, action: { url: `/org/${place.address}`, allowed: true, reason: null } },
    { id: 'teammate', state: 'open', fact: 'not_done', scripted: false, source, action: { url: `/org/${place.address}?tab=members`, allowed: true, reason: null } },
  ] };
}

beforeEach(() => {
  observed.reads.mockClear(); observed.requests.mockClear(); observed.data = facts();
  observed.save.mockReset().mockImplementation((choice) => Promise.resolve({ help: { first_steps: choice }, sidebar: { last_org_id: choice.org_id } }));
});
afterEach(async () => {
  cleanup();
  useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

describe.each(modes)('First steps in %s/%s', (locale, theme) => {
  async function appearanceForTest() {
    await setLocale(locale);
    useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
  }

  it('does not request setup for departments, members or administrators who do not own the root', async () => {
    await appearanceForTest();
    for (const place of [
      solo,
      { ...root, archived: true },
      { ...root, parent_id: 'parent', depth: 1 },
      { ...root, parent_id: 'parent', depth: 0 },
      { ...root, access: { ...root.access, owns: false, administers: true, admin_access: { place: 'system' as const, private: true } } },
      { ...root, access: { ...memberAccess, owns: false } },
    ]) {
      const view = show(<Checklist org={place} />);
      expect(screen.queryByTestId('first-steps')).not.toBeInTheDocument();
      expect(observed.reads).not.toHaveBeenCalled();
      view.unmount();
    }
  });

  it('shows a missing provider or teammate, but no card for complete or unknown setup', async () => {
    await appearanceForTest();
    for (const [provider, teammate, visible] of [
      ['open', 'done', true], ['done', 'open', true], ['done', 'done', false], ['unknown', 'unknown', false],
    ] as const) {
      const data = facts();
      data.steps = data.steps.map((step) => step.id === 'provider'
        ? { ...step, state: provider, fact: provider === 'done' ? 'key_saved' : provider === 'unknown' ? 'unknown' : 'not_done' }
        : step.id === 'teammate' ? { ...step, state: teammate, fact: teammate === 'done' ? 'member_joined' : teammate === 'unknown' ? 'unknown' : 'not_done' } : step);
      observed.data = data;
      const view = show(<Checklist org={root} />);
      expect(screen.queryByTestId('first-steps') !== null).toBe(visible);
      view.unmount();
    }
    for (const work_mode of [null, 'solo'] as const) {
      observed.data = { ...facts(solo), work_mode, steps: facts(solo).steps.map((step) => step.id === 'provider' ? { ...step, state: 'done', fact: 'key_saved' } : step) };
      const view = show(<Checklist org={solo} />);
      expect(screen.queryByTestId('first-steps')).not.toBeInTheDocument();
      view.unmount();
    }
    observed.data = { ...facts(), steps: facts().steps.map((step) => step.id === 'provider' ? { ...step, state: 'done', fact: 'key_saved' } : step) };
    show(<Checklist org={{ ...root, member_count: 5 }} />);
    expect(screen.queryByTestId('first-steps')).not.toBeInTheDocument();
  });

  it('says its title once, omits the org/check time/Refresh and leaves project creation to the page', async () => {
    await appearanceForTest();
    show(<Checklist org={root} />);
    const card = screen.getByTestId('first-steps');
    expect(within(card).getAllByText(i18n.t('firstSteps.title'))).toHaveLength(1);
    expect(within(card).queryByText(root.name)).not.toBeInTheDocument();
    expect(card).not.toHaveTextContent(/Checked|检查时间|上次检查|2026-10-15/i);
    expect(within(card).queryByRole('button', { name: /Refresh|刷新/ })).not.toBeInTheDocument();
    expect(within(card).queryByRole('link', { name: i18n.t('firstSteps.actions.work') })).not.toBeInTheDocument();
    expect(card.querySelector('[data-step="team"]')).not.toBeInTheDocument();
    for (const action of card.querySelectorAll('button, a')) expect(action).toHaveClass('h-(--control-sm)');
  });

  it('drops the card immediately when current ownership changes and refuses another org’s projection', async () => {
    await appearanceForTest();
    const view = show(<Checklist org={root} />);
    expect(screen.getByTestId('first-steps')).toBeVisible();
    const reads = observed.reads.mock.calls.length;
    view.rerender(<Checklist org={{ ...root, access: { ...root.access, owns: false } }} />);
    expect(screen.queryByTestId('first-steps')).not.toBeInTheDocument();
    expect(observed.reads).toHaveBeenCalledTimes(reads);
    view.unmount();
    observed.data = facts(solo);
    show(<Checklist org={root} />);
    expect(screen.queryByTestId('first-steps')).not.toBeInTheDocument();
  });

  it('keeps a new team’s mode actions compact while own spaces mount no setup reader', async () => {
    await appearanceForTest();
    observed.data = { ...facts(), work_mode: null };
    show(<Checklist org={root} />);
    const card = screen.getByTestId('first-steps');
    const chooseTeam = within(card).getByRole('button', { name: i18n.t('firstSteps.useTeam') });
    const workAlone = within(card).getByRole('button', { name: i18n.t('firstSteps.workAlone') });
    expect(chooseTeam.parentElement).toBe(workAlone.parentElement);
    expect(chooseTeam.parentElement).toHaveClass('flex-wrap');
    expect(chooseTeam).not.toHaveClass('w-full');
    expect(within(card).queryByText(i18n.t('firstSteps.steps.team'))).not.toBeInTheDocument();
    expect(card.querySelector('details')).not.toBeInTheDocument();
    for (const action of card.querySelectorAll('button, a')) expect(action).toHaveClass('h-(--control-sm)');
  });

  it('uses the save answer immediately for the mode and dismissal without a setup reread', async () => {
    await appearanceForTest();
    observed.data = { ...facts(), work_mode: null };
    show(<Checklist org={root} />);
    fireEvent.click(screen.getByRole('button', { name: i18n.t('firstSteps.useTeam') }));
    expect(await screen.findByText(i18n.t('firstSteps.progress'))).toBeVisible();
    expect(observed.save).toHaveBeenCalledTimes(1);
    expect(observed.requests).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: i18n.t('firstSteps.dismiss') }));
    expect(await screen.findByRole('button', { name: i18n.t('firstSteps.show') })).toBeVisible();
    expect(observed.save).toHaveBeenLastCalledWith({ org_id: root.id, work_mode: 'team', dismissed: true });
    expect(observed.save).toHaveBeenCalledTimes(2);
    expect(observed.requests).not.toHaveBeenCalled();
  });

  it('gives a blocked action one readable access explanation without a raw permission key', async () => {
    await appearanceForTest();
    observed.data = { ...facts(), steps: facts().steps.map((step) => step.id === 'provider'
      ? { ...step, action: { url: '/resources/providers', allowed: false, reason: null } } : step) };
    show(<Checklist org={root} />);
    expect(screen.getByRole('button', { name: i18n.t('firstSteps.actions.provider') })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getAllByText(i18n.t('firstSteps.accessNeeded'))).toHaveLength(1);
    expect(screen.getByTestId('first-steps')).not.toHaveTextContent('settings.manage');
  });
});
