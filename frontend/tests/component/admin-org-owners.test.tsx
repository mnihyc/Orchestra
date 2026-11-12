/** Owner recovery remains reachable when the Organizations table folds its columns. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { Organizations } from '@/features/admin/Organizations';
import type { Org } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

import { show } from './support';

const rows = vi.hoisted(() => ({ active: [] as Org[], deleted: [] as Org[] }));
vi.mock('@/api/admin', () => ({
  adminOrgsQuery: (filter: { deleted: boolean }) => ({
    queryKey: ['admin', 'orgs', filter], initialPageParam: null,
    queryFn: () => Promise.resolve({ items: filter.deleted ? rows.deleted : rows.active, next_cursor: null }),
    getNextPageParam: () => undefined,
  }),
  orgPeopleQuery: (id: string, q: string) => ({ queryKey: ['org-people', id, q], queryFn: () => Promise.resolve({ items: [], next_cursor: null }) }),
  nameOwner: vi.fn(), restoreOrg: vi.fn(), setOrgLimits: vi.fn(),
}));

let room = 400;
const observers = new Set<() => void>();
beforeEach(() => {
  room = 400;
  vi.spyOn(Element.prototype, 'clientWidth', 'get').mockImplementation(function (this: Element) {
    return this.getAttribute('data-scroll-allow') === 'table' ? room : 0;
  });
  vi.stubGlobal('ResizeObserver', class implements ResizeObserver {
    private readonly report: () => void;
    constructor(callback: ResizeObserverCallback) { this.report = () => callback([], this); }
    observe() { observers.add(this.report); }
    unobserve() { observers.delete(this.report); }
    disconnect() { observers.delete(this.report); }
  });
});
afterEach(async () => {
  cleanup(); observers.clear(); vi.restoreAllMocks(); vi.unstubAllGlobals(); await setLocale('en');
});

const org = (id: string, name: string): Org => ({
  id, name, kind: 'org', org_kind: 'team', org_id: id, parent_id: null, depth: 0, slug: id, address: id,
  description: '', readme: null, color: null, title_source: 'user', tags: [], tags_source: 'user', auto_updated_at: null,
  path: [{ id, kind: 'org', name, redacted: false }], owner: { id: 'former-owner', kind: 'human', username: 'former', display_name: 'Former owner', avatar: { text: 'F', color: 1 } },
  owners: 0, page: 'follows', access: { permissions: [], admin_only: [], participating: false, administers: true, owns: false, role: null, role_from: null, subscription: null, effective_subscription: 'off' },
  archived: false, archived_at: null, archived_by: null, chat_id: 'team-chat', closed: false, flat: false,
  member_count: 1, org_count: 0, project_count: 0, revision: 1,
  created_at: '2026-11-01T00:00:00Z', updated_at: '2026-11-01T00:00:00Z', changed_at: '2026-11-01T00:00:00Z',
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps owner recovery available once across table widths and leaves deleted orgs read only (${locale})`, async () => {
  await setLocale(locale);
  rows.active = [org('unowned', 'No owner team')];
  rows.deleted = [org('deleted', 'Deleted team')];
  const user = userEvent.setup();
  show(<Organizations search={{}} onSearch={() => undefined} />);
  const table = await screen.findByRole('table', { name: i18n.t('admin.sections.orgs.title') });
  const label = i18n.t('admin.orgs.owner.open');
  expect(within(table).queryByRole('columnheader', { name: i18n.t('admin.orgs.owners') })).not.toBeInTheDocument();
  const recover = within(table).getByRole('button', { name: label });
  expect(recover).toBeVisible();
  const deleted = await screen.findByRole('row', { name: /Deleted team/ });
  expect(within(deleted).queryByRole('button', { name: label })).not.toBeInTheDocument();
  recover.focus();
  await user.keyboard('{Enter}');
  const dialog = await screen.findByRole('dialog', { name: i18n.t('admin.orgs.owner.title', { org: 'No owner team' }) });
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.cancel') }));
  act(() => { room = 1440; for (const report of observers) report(); });
  expect(within(table).getByRole('columnheader', { name: i18n.t('admin.orgs.owners') })).toBeVisible();
  expect(within(table).getAllByRole('button', { name: label })).toHaveLength(1);
  expect(within(deleted).queryByRole('button', { name: label })).not.toBeInTheDocument();
});
