/** Real personal/place form components retain Saved and Undo across their accepted name/revision changes. */
import { useQuery } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Nodes from '@/api/nodes';
import { orgBySlugKey } from '@/api/nodes';
import { Profile } from '@/features/settings/personal/Profile';
import { General } from '@/features/settings/org/General';
import type { Me } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { IDENTITY_COLORS } from '@/lib/grapheme';
import { me, memberAccess, meta, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ me: vi.fn<() => Promise<Me>>(), writeMe: vi.fn<typeof Auth.updateMe>(), writeNode: vi.fn<typeof Nodes.updateNode>(), readNode: vi.fn<typeof Nodes.readNode>() }));
const person = me(1, 'ada', 'Ada Lovelace');
const team = org('01000000-0000-4000-8000-000000000001', 'acme', 'Acme', userRef(person), { revision: 4,
  access: { ...memberAccess, owns: true, permissions: ['org.view', 'org.manage'] } });
const appearance = useAppearance.getState();
vi.mock('@/api/auth', async (load) => {
  const actual = await load<typeof Auth>();
  return { ...actual, updateMe: calls.writeMe,
    meQuery: { queryKey: actual.meKey, initialData: () => person, queryFn: calls.me, staleTime: Infinity },
    metaQuery: { queryKey: ['meta'], initialData: () => meta(), queryFn: () => Promise.resolve(meta()), staleTime: Infinity } };
});
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(), updateNode: calls.writeNode, readNode: calls.readNode }));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn() }));
beforeEach(() => { vi.clearAllMocks(); calls.writeMe.mockReset(); calls.writeNode.mockReset(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function PlaceForm() {
  const query = useQuery({ queryKey: orgBySlugKey('acme'), initialData: team, queryFn: () => Promise.resolve(team), staleTime: Infinity });
  return <General node={query.data} />;
}

it('counts personal names as Unicode scalars, preserving a valid name longer than 64 UTF-16 units', async () => {
  const displayName = '😀'.repeat(40);
  calls.writeMe.mockResolvedValue({ ...person, display_name: displayName, avatar: { ...person.avatar, text: '😀' } });
  show(<Profile />);
  const name = screen.getByRole('textbox', { name: i18n.t('profile.name.label') });
  fireEvent.change(name, { target: { value: displayName } });
  await waitFor(() => expect(calls.writeMe).toHaveBeenCalledExactlyOnceWith({ display_name: displayName }));
  expect(name).toHaveValue(displayName);
});

it('serializes personal name and colour writes so older complete Me answers cannot replace a newer choice', async () => {
  let accept: (value: Me) => void = () => { throw new Error('The name write has not started'); };
  const renamed = { ...person, display_name: 'Zeta Lovelace', avatar: { ...person.avatar, text: 'Z' } };
  calls.writeMe.mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }))
    .mockResolvedValueOnce({ ...renamed, avatar: { ...renamed.avatar, color: 2 } });
  const user = userEvent.setup(); show(<Profile />);
  const name = screen.getByRole('textbox', { name: i18n.t('profile.name.label') });
  fireEvent.change(name, { target: { value: renamed.display_name } });
  await waitFor(() => expect(calls.writeMe).toHaveBeenCalledTimes(1));
  const colour = screen.getByRole('radio', { name: i18n.t('profile.avatar.color', { number: 3 }) });
  await user.click(colour);
  expect(calls.writeMe).toHaveBeenCalledTimes(1);
  await act(async () => { accept(renamed); await Promise.resolve(); });
  await waitFor(() => expect(calls.writeMe).toHaveBeenCalledTimes(2));
  expect(calls.writeMe).toHaveBeenLastCalledWith({ avatar_color: 2 });
  expect(name).toHaveValue(renamed.display_name);
  await screen.findByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: i18n.t('profile.avatar.label') }) });
  expect(screen.getByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: i18n.t('profile.name.label') }) })).toBeVisible();
});

for (const [locale, theme] of [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const) {
  it(`saves the ringed avatar choice once and resets through the native colour answer in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const chosen = (person.avatar.color + 1) % IDENTITY_COLORS;
    let accept: (value: Me) => void = () => { throw new Error('The colour write has not started'); };
    calls.writeMe.mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; })).mockResolvedValueOnce(person);
    const user = userEvent.setup(); show(<Profile />);
    const group = screen.getByRole('radiogroup', { name: i18n.t('profile.avatar.label') });
    const previous = within(group).getByRole('radio', { name: i18n.t('profile.avatar.color', { number: person.avatar.color + 1 }) });
    const next = within(group).getByRole('radio', { name: i18n.t('profile.avatar.color', { number: chosen + 1 }) });
    expect(within(group).getAllByRole('radio')).toHaveLength(IDENTITY_COLORS);
    expect(previous).toBeChecked();
    expect(screen.getAllByText(i18n.t('profile.avatar.label'), { exact: true })).toHaveLength(1);
    expect(calls.writeMe).not.toHaveBeenCalled();
    await user.click(next);
    await waitFor(() => expect(calls.writeMe).toHaveBeenCalledExactlyOnceWith({ avatar_color: chosen }));
    expect(next).toBeChecked();
    expect(next).toBeDisabled();
    expect(previous).not.toBeChecked();
    expect(next.querySelector('svg, .absolute')).toBeNull();
    await act(async () => { accept({ ...person, avatar: { ...person.avatar, color: chosen } }); await Promise.resolve(); });
    await waitFor(() => expect(next).toBeEnabled());
    await user.click(next);
    expect(calls.writeMe).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: i18n.t('profile.avatar.reset') }));
    await waitFor(() => expect(previous).toBeChecked());
    expect(calls.writeMe).toHaveBeenCalledTimes(2);
    expect(calls.writeMe).toHaveBeenLastCalledWith({ avatar_color: null });
    expect(calls.me).not.toHaveBeenCalled();
  });

  it(`keeps a refused avatar choice reviewable and retries its exact colour in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const chosen = (person.avatar.color + 1) % IDENTITY_COLORS;
    calls.writeMe.mockRejectedValueOnce(new Error('The colour could not be saved.'))
      .mockResolvedValueOnce({ ...person, avatar: { ...person.avatar, color: chosen } });
    const user = userEvent.setup(); show(<Profile />);
    const group = screen.getByRole('radiogroup', { name: i18n.t('profile.avatar.label') });
    const next = within(group).getByRole('radio', { name: i18n.t('profile.avatar.color', { number: chosen + 1 }) });
    await user.click(next);
    const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
    expect(next).toBeChecked();
    expect(next).toBeEnabled();
    expect(next.querySelector('svg, .absolute')).toBeNull();
    expect(calls.writeMe).toHaveBeenCalledExactlyOnceWith({ avatar_color: chosen });
    expect(screen.queryByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: i18n.t('profile.avatar.label') }) })).not.toBeInTheDocument();
    await user.click(retry);
    await waitFor(() => expect(calls.writeMe).toHaveBeenCalledTimes(2));
    expect(calls.writeMe).toHaveBeenLastCalledWith({ avatar_color: chosen });
    await screen.findByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: i18n.t('profile.avatar.label') }) });
    expect(next).toBeChecked();
    expect(calls.me).not.toHaveBeenCalled();
  });

  it(`keeps personal name feedback and its inverse after the canonical name changes in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    calls.writeMe.mockResolvedValueOnce({ ...person, display_name: 'Zeta Lovelace', avatar: { ...person.avatar, text: 'Z' } }).mockResolvedValueOnce(person);
    const user = userEvent.setup(); show(<Profile />);
    const name = screen.getByRole('textbox', { name: i18n.t('profile.name.label') });
    await user.clear(name); await user.type(name, 'Zeta Lovelace');
    // The mutation receipt can precede the query observer's canonical value. Undo waits for both;
    // query the ready control rather than retaining its earlier, blocked tooltip trigger.
    const undo = await waitFor(() => {
      const control = screen.getByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: i18n.t('profile.name.label') }) });
      expect(control).toBeEnabled();
      expect(control).not.toHaveAttribute('aria-disabled', 'true');
      return control;
    });
    expect(name).toHaveValue('Zeta Lovelace');
    expect(screen.getByText(i18n.t('settings.compact.saved'), { exact: true })).toBeVisible();
    expect(calls.writeMe).toHaveBeenCalledExactlyOnceWith({ display_name: 'Zeta Lovelace' });
    await user.click(undo);
    await waitFor(() => expect(name).toHaveValue('Ada Lovelace'));
    expect(calls.writeMe).toHaveBeenLastCalledWith({ display_name: 'Ada Lovelace' });
    expect(calls.me).not.toHaveBeenCalled();
  });

  it(`keeps place identity feedback after revision advancement and sends Undo with that accepted revision in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    calls.writeNode.mockResolvedValueOnce({ ...team, name: 'New team', revision: 5 }).mockResolvedValueOnce({ ...team, revision: 6 });
    const user = userEvent.setup(); show(<PlaceForm />);
    const name = screen.getByRole('textbox', { name: i18n.t('settings.general.name') });
    await user.clear(name); await user.type(name, 'New team');
    const undo = await waitFor(() => {
      const control = screen.getByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: i18n.t('settings.general.identity') }) });
      expect(control).toBeEnabled();
      expect(control).not.toHaveAttribute('aria-disabled', 'true');
      return control;
    });
    expect(name).toHaveValue('New team');
    expect(calls.writeNode).toHaveBeenCalledExactlyOnceWith(team.id, { name: 'New team', expected_revision: 4 });
    await user.click(undo);
    await waitFor(() => expect(name).toHaveValue('Acme'));
    expect(calls.writeNode).toHaveBeenLastCalledWith(team.id, { name: 'Acme', expected_revision: 5 });
    expect(calls.readNode).not.toHaveBeenCalled();
  });
}
