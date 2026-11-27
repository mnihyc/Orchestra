/** The existing destination form carries a selected sub-agent kind with its catalogue name. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import { QueryClient } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Settings from '@/api/settings';
import { Destinations } from '@/features/settings/Destinations';
import type { Destination, Notifier } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { id, MADE_AT, me, memberAccess, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  read: vi.fn<() => Promise<Destination[]>>(),
  create: vi.fn<typeof Settings.createDestination>(),
  update: vi.fn<typeof Settings.updateDestination>(),
}));
const node = org(id(2, 700), 'team', 'Team', userRef(me(700, 'reader', 'Reader')), {
  access: { ...memberAccess, permissions: ['node.view', 'settings.manage'] },
});
const notifier: Notifier = {
  name: 'relay', title: 'Relay', summary: 'Sends to a team room.', links: false, single: false,
  owners: [], targets: ['destination'], fields: [],
  templates: { single: { subject: null, body: '{{title}}' }, digest: { subject: null, body: '{{count}} notifications' } },
};
const stored: Destination = {
  id: id(5, 700), channel: 'relay', name: 'Team room',
  node: { id: node.id, kind: 'org', name: node.name, redacted: false },
  config: {}, secrets_set: [], templates: {}, kinds: ['turn.failed'], level: 'all', enabled: true,
  last_ok_at: null, last_error: null, created_at: MADE_AT,
};
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(),
  destinationsQuery: (nodeId: string) => ({ queryKey: ['destinations', nodeId], queryFn: calls.read }),
  destinationsPageQuery: (nodeId: string, q: string) => ({ queryKey: ['destinations', nodeId, 'pages', q], initialPageParam: null,
    queryFn: async () => ({ items: (await calls.read()).filter((row) => row.name.includes(q)).slice(0, 37), next_cursor: null }), getNextPageParam: () => null }),
  notifiersQuery: { queryKey: ['notifiers'], queryFn: () => Promise.resolve([notifier]) },
  createDestination: calls.create, updateDestination: calls.update,
}));
beforeEach(() => {
  calls.read.mockReset().mockResolvedValue([]);
  calls.create.mockReset().mockResolvedValue({ ...stored, kinds: ['subagent.ended'] });
  calls.update.mockReset().mockResolvedValue({ ...stored, kinds: ['subagent.ended'] });
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it('keeps a destination draft when a native page no longer contains its row', async () => {
  calls.read.mockResolvedValue([stored]);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  show(<Destinations node={node} />, cache);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.channels.editNamed', { name: stored.name }) }));
  const dialog = screen.getByRole('dialog');
  const name = within(dialog).getByRole('textbox', { name: i18n.t('settings.channels.name') });
  await user.clear(name); await user.paste('Unfinished destination');
  act(() => { cache.setQueryData(['destinations', node.id, 'pages', ''], { pages: [{ items: [], next_cursor: null }], pageParams: [null] }); });
  expect(dialog).toBeVisible(); expect(name).toHaveValue('Unfinished destination');
});

it('finds a destination beyond a thousand by its server name search', async () => {
  calls.read.mockResolvedValue(Array.from({ length: 1_001 }, (_, index) => ({ ...stored, id: id(6, index + 1), name: 'Destination ' + String(index).padStart(4, '0') })));
  show(<Destinations node={node} />);
  const user = userEvent.setup(), search = await screen.findByRole('searchbox');
  await user.click(search); await user.paste('Destination 1000');
  expect(await screen.findByRole('heading', { name: 'Destination 1000' })).toBeVisible();
  expect(screen.queryByRole('heading', { name: 'Destination 0000' })).toBeNull();
});

for (const locale of ['en', 'zh-CN'] as const) for (const mode of ['create', 'edit'] as const) {
  it(`saves only the selected sub-agent kind through the existing ${mode} form in ${locale}`, async () => {
    await setLocale(locale);
    if (mode === 'edit') calls.read.mockResolvedValue([stored]);
    const user = userEvent.setup();
    show(<Destinations node={node} />);
    await waitFor(() => expect(calls.read).toHaveBeenCalledOnce());
    if (mode === 'create') await screen.findByText(i18n.t('settings.destinations.none', { name: node.name }));
    const open = mode === 'create'
      ? await screen.findByRole('button', { name: i18n.t('settings.destinations.add') })
      : await screen.findByRole('button', { name: i18n.t('settings.channels.editNamed', { name: stored.name }) });
    await user.click(open);
    const dialog = screen.getByRole('dialog');
    if (mode === 'create') {
      await user.type(within(dialog).getByRole('textbox', { name: i18n.t('settings.channels.name') }), stored.name);
      await user.click(within(dialog).getByRole('radio', { name: i18n.t('choiceCatalog.notificationKinds.some.name') }));
    } else {
      await user.click(within(dialog).getByRole('checkbox', { name: i18n.t('choiceCatalog.eventKind.turn_failed.name') }));
    }
    await user.click(within(dialog).getByRole('radio', { name: i18n.t('choiceCatalog.notificationLevel.all.name') }));
    const selected = within(dialog).getByRole('checkbox', { name: i18n.t('inbox.kinds.subagent_ended') });
    expect(selected).not.toBeChecked();
    await user.click(selected);
    expect(selected).toBeChecked();
    const submit = within(dialog).getByRole('button', {
      name: i18n.t(mode === 'create' ? 'settings.destinations.make' : 'settings.save'),
    });
    await user.click(selected);
    await user.click(submit);
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.update).not.toHaveBeenCalled();
    for (const hint of within(dialog).getAllByText(i18n.t('settings.destinations.tickOne'))) expect(hint).toBeVisible();
    await user.click(selected);
    await user.click(submit);
    const body = { name: stored.name, config: {}, kinds: ['subagent.ended'], level: 'all' };
    await waitFor(() => {
      if (mode === 'create') expect(calls.create).toHaveBeenCalledExactlyOnceWith(node.id, 'relay', body, expect.any(String));
      else expect(calls.update).toHaveBeenCalledExactlyOnceWith(stored.id, body);
    });
  });
}
