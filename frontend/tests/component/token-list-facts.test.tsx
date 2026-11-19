/** Token facts stay readable when responsive columns move into the identity cell. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as UsersApi from '@/api/users';
import type * as ReferencesApi from '@/api/references';
import type * as Router from '@tanstack/react-router';
import type { ReactNode } from 'react';
import { Tokens } from '@/features/settings/personal/Tokens';
import type { ApiTokenInfo, ReferencePreview } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

const rows: ApiTokenInfo[] = [
  { id: 'all', name: 'All work', scopes: ['*'], node: null, created_at: '2026-10-01T12:00:00Z', expires_at: '2030-01-31T12:00:00Z', last_used_at: null },
  { id: 'some', name: 'Read release', scopes: ['thread.view', 'topic.interact'], node: { id: 'web', kind: 'project', name: 'Web', redacted: false }, created_at: '2026-10-01T12:00:00Z', expires_at: null, last_used_at: null },
  { id: 'hidden', name: 'Limited work', scopes: ['thread.view'], node: { id: 'hidden-place', kind: 'project', name: null, redacted: true }, created_at: '2026-10-01T12:00:00Z', expires_at: null, last_used_at: null },
];
const calls = vi.hoisted(() => ({ preview: vi.fn<() => Promise<ReferencePreview>>() }));
const preview: ReferencePreview = { id: 'web', kind: 'node', node_kind: 'project', title: 'Web', place: null, description: '', updated_at: null, updated_by: null, part: null };
vi.mock('@/api/users', async (load) => ({ ...await load<typeof UsersApi>(), tokensQuery: { queryKey: ['token-list-facts'], queryFn: () => Promise.resolve(rows) } }));
vi.mock('@/api/references', async (load) => {
  const actual = await load<typeof ReferencesApi>();
  return { ...actual, referencePreviewQuery: (reference: Parameters<typeof actual.referencePreviewQuery>[0]) => ({ ...actual.referencePreviewQuery(reference), queryFn: calls.preview }) };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ to, children }: { to: string; children: ReactNode }) => <a href={to}>{children}</a> }));
const appearance = useAppearance.getState();
beforeEach(() => { calls.preview.mockReset().mockResolvedValue(preview); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps reach, scopes, use and expiry when the pane is narrow in ${locale}/${theme}`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  sessionStorage.clear();
  show(<Tokens />);
  const all = await screen.findByRole('row', { name: /All work/ });
  expect(all).toHaveTextContent(i18n.t('tokens.scopes.everything'));
  expect(all).toHaveTextContent(i18n.t('tokens.everywhere'));
  expect(all).toHaveTextContent(i18n.t('tokens.neverUsed'));
  expect(all).toHaveTextContent('2030');
  expect(all.querySelector('time[datetime="2030-01-31T12:00:00Z"]')).toBeVisible();
  const some = screen.getByRole('row', { name: /Read release/ });
  expect(some).toHaveTextContent('Web');
  expect(within(some).getByRole('link', { name: /^Web$/ })).toHaveAttribute('href', '/project/web');
  expect(some).toHaveTextContent(i18n.t('tokens.scopes.count', { count: 2 }));
  expect(within(some).getByText(i18n.t('tokens.neverEnds'), { exact: true })).toBeVisible();
  const hidden = screen.getByRole('row', { name: /Limited work/ });
  expect(hidden).toHaveTextContent(i18n.t('people.privateReference'));
  expect(within(hidden).queryByRole('link')).not.toBeInTheDocument();
  expect(calls.preview).not.toHaveBeenCalled();
});

for (const opening of ['hover', 'focus', 'tap'] as const) it(`opens the token place's shared card by ${opening} without an earlier preview read`, async () => {
  const user = userEvent.setup();
  show(<Tokens />);
  const token = await screen.findByRole('row', { name: /Read release/ });
  const place = within(token).getByRole('link', { name: /^Web$/ });
  expect(calls.preview).not.toHaveBeenCalled();
  if (opening === 'hover') await user.hover(place);
  else if (opening === 'focus') act(() => { place.focus(); });
  else await user.click(place);
  await waitFor(() => expect(document.querySelector('[data-reference-card]')).toHaveTextContent('Web'), { timeout: 2_000 });
  expect(screen.getByRole('button', { name: (name) => name === i18n.t('people.reference.open') })).toBeVisible();
  expect(calls.preview).toHaveBeenCalledTimes(1);
  if (opening !== 'hover') {
    if (opening === 'focus') await user.keyboard('{Enter}');
    await user.keyboard('{Escape}');
    await waitFor(() => expect(document.querySelector('[data-reference-card]')).toBeNull());
  }
});
