/** The common add form keeps identity, level and Add in one row; optional controls retain their exact grants. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Candidates from '@/features/access/candidates';
import { AddHolders } from '@/features/access/AddHolders';
import type { Changes } from '@/features/access/changes';
import type { PageChange } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { BEA, CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ candidates: vi.fn() }));
vi.mock('@/features/access/candidates', async (original) => ({ ...await original<typeof Candidates>(), useTargetOrg: () => 'acme',
  useCandidates: (...args: Parameters<typeof Candidates.useCandidates>): Candidates.Candidates => {
    calls.candidates(...args);
    const person: Candidates.Candidate = { input: { kind: 'user', id: BEA.id }, holder: { kind: 'user', user: BEA }, name: 'Bea', outside: false, user: BEA };
    return { groups: [{ id: 'people', label: 'People', options: [{ value: 'bea', label: 'Bea', text: 'Bea' }] }], byValue: new Map([['bea', person]]), loading: false, placePicker: <div data-testid="source-place" /> };
  },
}));
vi.mock('@/features/access/Tell', () => ({ useDirectOff: () => true, TellThem: () => null, tellable: () => [], tellThem: () => Promise.resolve([]) }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
afterEach(async () => {
  cleanup(); vi.clearAllMocks(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});
it.each(modes)('starts compact and sends the chosen person/level without opening optional source reads in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const page = pageFor('document');
  const writes: PageChange[] = [];
  const changes: Changes = { busy: false, doing: null, error: null, stale: null, forget: () => undefined, again: () => Promise.resolve(false), run: (request) => {
    writes.push(request.make(page)); return Promise.resolve(true);
  } };
  show(<AddHolders target={{ kind: 'document', id: 'document', name: 'Release', orgId: 'acme' }} page={page} catalogue={CATALOGUE} changes={changes} />);
  const user = userEvent.setup();
  expect(screen.getByRole('combobox', { name: i18n.t('access.add.choice') })).toBeVisible();
  expect(screen.queryByTestId('source-place')).not.toBeInTheDocument();
  expect(screen.queryByLabelText(i18n.t('access.add.untilLabel'))).not.toBeInTheDocument();
  expect(calls.candidates).toHaveBeenLastCalledWith('', 'acme', expect.objectContaining({ browsePlaces: false }));
  const options = screen.getByRole('button', { name: i18n.t('access.roster.options') });
  await user.click(options);
  expect(options).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByTestId('source-place')).toBeVisible();
  expect(screen.getByLabelText(i18n.t('access.add.untilLabel'))).toBeVisible();
  await user.click(options);
  const field = screen.getByRole('combobox', { name: i18n.t('access.add.label') });
  await user.type(field, 'Bea');
  await user.click(await screen.findByRole('option', { name: 'Bea' }));
  await user.click(screen.getByRole('button', { name: i18n.t('access.add.button') }));
  await waitFor(() => expect(writes).toHaveLength(1));
  expect(writes[0]).toMatchObject({ expected_revision: page.revision, name: [{ choice: 'full_view', holders: [{ kind: 'user', id: BEA.id }] }] });
});
