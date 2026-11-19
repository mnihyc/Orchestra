/** D443 S58: one dialog name, a distinct field label and readable recent identities with their original destinations. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import type * as Sources from '@/features/finder/sources';
import { Finder } from '@/features/finder/Finder';
import { rowOfName, type Env, type GroupId } from '@/features/finder/rows';
import { openFinder, useFinder } from '@/features/finder/store';
import { hintOf } from '@/features/help/hints';
import type { NameHit, NodeRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';
const calls = vi.hoisted(() => ({ go: vi.fn<Env['go']>() }));
const root: NodeRef = { id: 'acme', kind: 'org', name: 'Acme', redacted: false };
const hit = (more: Partial<NameHit> & Pick<NameHit, 'id' | 'kind' | 'name'>): NameHit => ({ href: null, tag: null, thread_attention: null, color: null, custom_color: null, address: null, username: null, avatar: null, path: [], owner: null, archived: false, recent: true, ...more });
const recents = [
  hit({ id: 'alice', kind: 'user', name: 'Alice Archer', username: 'alice', address: 'acme.alice', href: '/u/alice' }),
  hit({ id: root.id, kind: 'org', name: 'Acme', address: 'acme', path: [root], href: '/org/acme' }),
  hit({ id: 'crew', kind: 'group', name: 'Release crew', address: 'acme.crew', path: [root] }),
  hit({ id: 'gpu', kind: 'provider', name: 'GPU model', address: 'acme.ada.gpu', path: [root], href: '/resources/providers/gpu' }),
];
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), orgsQuery: { queryKey: ['human-finder-orgs'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => () => undefined }));
vi.mock('@/features/finder/sources', async (original) => ({ ...await original<typeof Sources>(),
  useEnv: (showCard: Env['showCard']): Env => ({ t: i18n.t, go: calls.go, orgSlug: 'acme', exists: () => true, showCard }),
  useAnswers: (input: Sources.AnswerInput): ReturnType<typeof Sources.useAnswers> => ({
    answers: input.active ? { recent: recents.map((name) => ({ row: rowOfName(name, 'recent', input.query, input.env), score: 1 })) } : {},
    full: new Set<GroupId>(), partial: false, pending: false, failure: null,
  }),
}));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const finderBefore = useFinder.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); useFinder.setState({ ...finderBefore, open: false, participating: false }); });
afterEach(async () => {
  cleanup(); useFinder.setState(finderBefore); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});
it.each(modes)('names the field once, omits repeated guidance and presents recent people/places in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  openFinder();
  show(<Finder />);
  const dialog = screen.getByRole('dialog', { name: i18n.t('finder.title') });
  const field = within(dialog).getByRole('combobox', { name: i18n.t('finder.field') });
  expect(field).toHaveFocus();
  expect(field).toHaveAttribute('placeholder', i18n.t('finder.placeholder'));
  expect(within(dialog).getAllByText(i18n.t('finder.title'), { exact: true })).toHaveLength(1);
  expect(within(dialog).queryByText(hintOf('finder-modes', i18n.t).what)).not.toBeInTheDocument();
  const person = within(dialog).getByRole('option', { name: /Alice Archer/ });
  expect(person).toHaveTextContent('@alice');
  for (const value of ['acme.alice', 'acme.crew', 'acme.ada.gpu']) expect(dialog).not.toHaveTextContent(value);
  const company = within(dialog).getByText('Acme', { exact: true }).closest('[role="option"]');
  expect(company).not.toBeNull();
  expect(company).not.toHaveTextContent('acme');
  expect(within(dialog).getByRole('option', { name: /Release crew/ })).toHaveTextContent('Acme');
  expect(within(dialog).getByRole('option', { name: /GPU model/ })).toHaveTextContent('Acme');
  await userEvent.keyboard('{Enter}');
  await waitFor(() => expect(calls.go).toHaveBeenCalledExactlyOnceWith({ to: '/u/alice' }, 'here'));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: i18n.t('finder.title') })).not.toBeInTheDocument());
});
