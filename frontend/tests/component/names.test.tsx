/** Every name can be found: a person's name opens their card on a click and shows it under the keyboard's focus; a
 * row of avatars, and its "+3", opens the list of everybody in it; inside a button or a link a name is part of what
 * that says and opens nothing of its own, its whole name under the pointer. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';

import type { UserRef } from '@/generated/api-types';
import { AvatarStack } from '@/ui/AvatarStack';
import { UserChip } from '@/ui/UserChip';
import { Dialog } from '@/ui/Dialog';
import { PresenceStack } from '@/ui/PresenceStack';

import { show as showBare } from './support';

// What a person's card reads of their page is the profile's own test; here it is still on its way.
vi.mock('@/api/profile', async (original) => ({ ...await original<object>(), profileQuery: (username: string) => ({ queryKey: ['profile', username], queryFn: () => new Promise(() => undefined) }) }));

function show(ui: ReactNode) {
  return showBare(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);
}

const person = (id: string, name: string, kind: UserRef['kind'] = 'human'): UserRef => ({ id, display_name: name, username: name.toLowerCase().replaceAll(' ', '.'), kind, avatar: { text: name.slice(0, 1), color: 1 } });
const ada = person('ada', 'Ada Lovelace');
const robot = person('build', 'Build bot', 'service');
const crowd = [ada, person('grace', 'Grace Hopper'), person('dora', 'Dora Developer'), person('linus', 'Linus Pauling')];

describe('a person’s name', () => {
  it('opens their card on a click', async () => {
    show(<p>Owned by <UserChip user={ada} /></p>);
    await userEvent.click(screen.getByRole('button', { name: 'Ada Lovelace' }));
    expect(await screen.findByRole('dialog', { name: 'Ada Lovelace' })).toBeVisible();
  });

  it('shows the card under the keyboard’s focus', async () => {
    show(<p>Run by <UserChip user={robot} /></p>);
    await userEvent.tab();
    expect(screen.getByRole('button', { name: 'Build bot' })).toHaveFocus();
    // The passing card waits a moment before it shows, as it does under the pointer.
    expect(await screen.findByText('@build.bot', {}, { timeout: 2_000 })).toBeVisible();
  });

  it('opens nothing of its own inside a button, and keeps its whole name under the pointer', () => {
    show(<button type="button">Choose <UserChip user={ada} /></button>);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    expect(screen.getByText('Ada Lovelace')).toHaveAttribute('title', 'Ada Lovelace');
  });

  it('opens nothing where the screen gives the card itself', () => {
    show(<p><UserChip user={ada} card={false} /></p>);
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });
});

describe('a row of avatars', () => {
  it('keeps the people list and its dialog open when Escape dismisses only a nested person card', async () => {
    const closed = vi.fn(), user = userEvent.setup();
    show(<Dialog open onOpenChange={closed} title="Looking"><PresenceStack users={[{ since: '2026-10-25T00:00:00Z', user: { ...ada, username: '' } }]} /></Dialog>);
    await user.click(screen.getByRole('button', { name: 'People: Ada Lovelace' }));
    const list = await screen.findByRole('dialog', { name: 'People' });
    await user.click(within(list).getByRole('button', { name: ada.display_name }));
    const card = await screen.findByRole('dialog', { name: ada.display_name });
    expect(card).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: ada.display_name })).not.toBeInTheDocument();
    expect(list).toBeVisible();
    expect(closed).not.toHaveBeenCalled();
  });

  it('opens the known identity when a listed person has no profile locator', async () => {
    const { username: _username, ...known } = ada;
    show(<p><AvatarStack users={[known]} /></p>);
    await userEvent.click(screen.getByRole('button', { name: 'People: Ada Lovelace' }));
    const list = await screen.findByRole('dialog', { name: 'People' });
    await userEvent.click(within(list).getByRole('button', { name: ada.display_name }));
    const card = await screen.findByRole('dialog', { name: ada.display_name });
    expect(card).toHaveTextContent(ada.display_name);
    expect(card.querySelector('[aria-busy="true"]')).toBeNull();
    expect(card).not.toHaveTextContent('@');
  });

  it('opens the list of everybody in it, the ones counted as "+2" too, each opening their card', async () => {
    show(<p><AvatarStack users={crowd} max={2} /></p>);
    expect(screen.getByText('+2')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'People: Ada Lovelace, Grace Hopper, Dora Developer, Linus Pauling' }));
    const list = await screen.findByRole('dialog', { name: 'People' });
    for (const one of crowd) expect(within(list).getByRole('button', { name: one.display_name })).toBeVisible();
    await userEvent.click(within(list).getByRole('button', { name: 'Linus Pauling' }));
    expect(await screen.findByRole('dialog', { name: 'Linus Pauling' })).toBeVisible();
  });

  it('says how many more there are than it holds', async () => {
    show(<p><AvatarStack users={crowd.slice(0, 2)} total={8} max={2} /></p>);
    await userEvent.click(screen.getByRole('button', { name: 'People: Ada Lovelace, Grace Hopper' }));
    expect(await screen.findByText('and 6 more')).toBeVisible();
  });

  it('opens nothing of its own inside a link', () => {
    show(<a href="/org/acme"><AvatarStack users={crowd} max={2} /></a>);
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.getByRole('link')).toBeInTheDocument();
  });
});
