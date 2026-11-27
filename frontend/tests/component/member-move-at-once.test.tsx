/** A member is moved as soon as a place is chosen: the plan is read beside the choice, says only what is lost and holds
 * the move only by the server's refusal. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { moveMember, previewMemberMove } from '@/api/directory';
import { MoveMember } from '@/features/settings/org/MoveMember';
import type { Member, MemberMovePlan, Org } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { ACME, acmeTree } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/api/directory', async (original) => ({ ...await original<object>(), previewMemberMove: vi.fn(), moveMember: vi.fn() }));

const org = { id: ACME.org, org_id: ACME.org, kind: 'org', name: 'Acme' } as unknown as Org;
const grace = { user: { id: 'grace', display_name: 'Grace Hopper' }, domain: { id: ACME.org, name: 'Acme' }, role: 'member' } as unknown as Member;
const none = { nodes: 0, resources: 0, documents: 0 };
function plan(change: Partial<MemberMovePlan> = {}): MemberMovePlan {
  return { user: { id: 'grace', display_name: 'Grace Hopper' }, from: { id: ACME.org, kind: 'org', name: 'Acme' }, to: { id: ACME.ml, kind: 'org', name: 'ML' },
    role: 'member', loses: none, gains: { nodes: 4, resources: 1, documents: 2 }, stays: { roles: [{ id: 'role' }], nodes: { orgs: 0, projects: 1, topics: 2, threads: 3, documents: 0 } },
    missing: [], waiting: [], ...change } as unknown as MemberMovePlan;
}
const pending = () => new Promise<MemberMovePlan>(() => undefined);
/** As on the members list, closing removes the dialog: notices wait while a dialog is open. */
function Host({ onClose }: { onClose: () => void }) {
  const [open, setOpen] = useState(true);
  return open ? <MoveMember org={org} member={grace} onClose={() => { onClose(); setOpen(false); }} /> : null;
}

beforeEach(() => {
  vi.clearAllMocks();
  treeWriter.reset(ACME.org, acmeTree(), 100);
  vi.mocked(moveMember).mockResolvedValue({ ...grace, domain: { id: ACME.ml, name: 'ML' } } as unknown as Member);
});
afterEach(async () => { cleanup(); useTreeStore.setState({ byOrg: new Map() }); await setLocale('en'); });

async function choose(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('combobox', { name: i18n.t('settings.members.move.to') }));
  await user.click(await screen.findByRole('option', { name: /ML$/ }));
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`moves as soon as a place is chosen, before its plan is read (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    vi.mocked(previewMemberMove).mockImplementation(pending);
    const close = vi.fn();
    show(<Host onClose={close} />);
    const submit = screen.getByRole('button', { name: i18n.t('settings.members.move.submit', { name: 'Grace Hopper' }) });
    expect(submit).toHaveAttribute('aria-disabled', 'true');
    await choose(user);
    await waitFor(() => expect(previewMemberMove).toHaveBeenCalledWith(ACME.org, 'grace', ACME.ml, null, expect.anything()));
    expect(submit).not.toHaveAttribute('aria-disabled', 'true');
    await user.click(submit);
    await waitFor(() => expect(moveMember).toHaveBeenCalledExactlyOnceWith(ACME.org, 'grace', ACME.ml, null));
    expect(await screen.findByText(i18n.t('settings.members.move.done', { name: 'Grace Hopper', to: 'ML' }))).toBeInTheDocument();
    expect(close).toHaveBeenCalledOnce();
  });
}

it('says only what is lost: what they stop reaching, and nothing of what stays or is gained', async () => {
  const user = userEvent.setup();
  vi.mocked(previewMemberMove).mockResolvedValue(plan({ loses: { nodes: 2, resources: 0, documents: 1 } }));
  show(<MoveMember org={org} member={grace} onClose={vi.fn()} />);
  await choose(user);
  expect(await screen.findByText(/Grace Hopper stops reaching/)).toHaveTextContent('Grace Hopper stops reaching 2 places and 1 document');
  expect(screen.queryByText(/4 places/)).not.toBeInTheDocument();
});

it('a move that loses nothing says nothing, and a refusal the plan reads holds it with what is missing', async () => {
  const user = userEvent.setup();
  vi.mocked(previewMemberMove).mockResolvedValue(plan());
  const view = show(<MoveMember org={org} member={grace} onClose={vi.fn()} />);
  await choose(user);
  await waitFor(() => expect(previewMemberMove).toHaveBeenCalled());
  expect(screen.queryByText(/stops reaching/)).not.toBeInTheDocument();
  view.unmount();
  vi.mocked(previewMemberMove).mockResolvedValue(plan({ missing: [{ permissions: ['org.manage'], object: { kind: 'org', id: ACME.ml, name: 'ML' }, ask: [], ask_more: 0, closed: null, options: null, override: false }] as unknown as MemberMovePlan['missing'] }));
  show(<MoveMember org={org} member={grace} onClose={vi.fn()} />);
  await choose(user);
  expect(await screen.findByText(i18n.t('settings.members.move.missing'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('settings.members.move.submit', { name: 'Grace Hopper' }) })).toHaveAttribute('aria-disabled', 'true');
});
