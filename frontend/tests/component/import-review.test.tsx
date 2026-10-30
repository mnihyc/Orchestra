/** The review exposes the server's choices, people, blockers and source preview before import. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ImportReview } from '@/features/people/ImportReview';
import type { ImportPlan, ImportRequest } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { me, memberAccess, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ preview: vi.fn(), people: vi.fn() }));
vi.mock('@/api/imports', async (original) => ({ ...await original<object>(), previewImport: calls.preview }));
vi.mock('@/api/chats', async (original) => ({ ...await original<object>(), peopleQuery: (_org: string | null, q: string) => ({ queryKey: ['import-people', q], queryFn: () => calls.people(q) as Promise<unknown>, retry: false }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const person = userRef(me(1, 'ada', 'Ada Lovelace'));
const node = org('place', 'acme', 'Acme', person, { access: { ...memberAccess, administers: true } });
const request: ImportRequest = { client_id: 'act', source: { files: [{ name: 'notes.zip', blob: 'a'.repeat(64) }] }, destination: {}, options: {}, choices: {} };
const plan: ImportPlan = {
  node: { id: 'place', kind: 'org', name: 'Acme', redacted: false },
  source: { mode: 'package', entries: 1, bytes: 100, package: null },
  counts: { places: 0, folders: 0, documents: 1, chats: 0, threads: 0, files: 0, messages: 0, turns: 0, comments: 0, versions: 0 },
  actions: { create: 0, keep_both: 1, replace: 0, skip: 0 }, conflicts: 1, bytes: 100, limits: { max_bytes: 2048, max_items: 10 },
  authors: 'labels', access: { carried: false, allowed: true, restored: false }, blockers: [],
  people: [{ principal: 'source-person', username: 'ada', display_name: 'Ada Lovelace', matched: 'username', user: person, objects: 1 }],
  losses: [{ code: 'native_sessions', message: 'Native sessions', count: 0, object: null }],
  objects: { items: [{ source_id: 'source', kind: 'document', name: 'Plan', parent: null, path: ['Notes'], bytes: 100, action: 'keep_both', allowed: ['keep_both', 'replace', 'skip'], existing: { id: 'existing', kind: 'document', name: 'Plan', found_by: 'name', updated_at: null }, blocked: null, creator: 'source-person', created_at: null }], next_cursor: null },
};
const setup = (override: Partial<Parameters<typeof ImportReview>[0]> = {}) => {
  const handlers = { onMore: vi.fn(), onOptions: vi.fn(), onObject: vi.fn(), onPerson: vi.fn() };
  show(<ImportReview node={node} request={request} plan={plan} rows={plan.objects.items} hasMore={false} loadingMore={false} moreError={null} {...handlers} {...override} />);
  return handlers;
};

afterEach(async () => { calls.preview.mockReset(); calls.people.mockReset(); await setLocale('en'); });

it('shows the limits and server-approved item actions before starting', async () => {
  const user = userEvent.setup();
  const handlers = setup();
  expect(screen.getByText('1 item · 100 B of the 2 KiB limit')).toBeVisible();
  expect(screen.getByText('The package carries no sharing')).toBeVisible();
  expect(screen.getByRole('switch', { name: 'Sharing' })).toBeDisabled();
  await user.click(screen.getByRole('switch', { name: 'Comments' }));
  expect(handlers.onOptions).toHaveBeenCalledWith({ comments: false });
  await user.click(screen.getByRole('combobox', { name: 'Action for Plan' }));
  await user.click(screen.getByRole('option', { name: 'Replace' }));
  expect(handlers.onObject).toHaveBeenCalledExactlyOnceWith('source', 'replace');
});

it('reads a preview only when requested and filters the held page for conflicts', async () => {
  calls.preview.mockResolvedValue({ kind: 'document', name: 'Plan', text: 'A short preview', truncated: true });
  const user = userEvent.setup();
  const first = plan.objects.items[0];
  if (first === undefined) throw new Error('Fixture has no item');
  const handlers = setup({ rows: [...plan.objects.items, { ...first, source_id: 'new', name: 'New document', existing: null, blocked: null }] });
  expect(calls.preview).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Preview Plan' }));
  expect(await screen.findByText('A short preview')).toBeVisible();
  expect(screen.getByText('Shortened')).toBeVisible();
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('tab', { name: /Conflicts/ }));
  expect(screen.getByRole('table', { name: 'Conflicts' })).toHaveTextContent('Plan');
  expect(screen.queryByText('New document')).not.toBeInTheDocument();
  await user.click(screen.getByRole('tab', { name: 'Content' }));
  expect(screen.getByText('New document')).toBeVisible();
  expect(handlers.onMore).not.toHaveBeenCalled();
});

it('shows matched accounts as people and translates known losses', async () => {
  await setLocale('zh-CN');
  const user = userEvent.setup();
  setup();
  await user.click(screen.getByRole('tab', { name: '人员' }));
  expect(screen.getAllByText('Ada Lovelace').length).toBeGreaterThan(0);
  await user.click(screen.getByRole('tab', { name: /不会导入/ }));
  expect(screen.getByText('智能体的原生会话不会恢复；会话以历史记录导入')).toBeVisible();
});

it('does not claim a filtered page is empty while more results may match', async () => {
  const user = userEvent.setup();
  const handlers = setup({ rows: [], hasMore: true });
  expect(screen.getByText('More items may match')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Next page' }));
  await waitFor(() => expect(handlers.onMore).toHaveBeenCalledOnce());
});

it('compares expanded source bytes with the limit rather than an estimate of what will be added', () => {
  setup({ plan: { ...plan, source: { ...plan.source, bytes: 2048 }, bytes: 100, limits: { max_bytes: 1024, max_items: 10 } } });
  expect(screen.getByText('1 item · 2 KiB of the 1 KiB limit')).toBeVisible();
});

it('does not offer comments or versions that plain files do not carry', () => {
  setup({ plan: { ...plan, source: { ...plan.source, mode: 'files' } } });
  expect(screen.getByRole('switch', { name: 'Comments' })).toBeDisabled();
  expect(screen.getByRole('switch', { name: 'Versions' })).toBeDisabled();
  expect(screen.getByText(/Plain files carry no comments or versions/)).toBeVisible();
});

it('shows a fixed action without offering a choice when the server allows no alternative', () => {
  const first = plan.objects.items[0];
  if (first === undefined) throw new Error('Fixture has no item');
  const fixed = { ...first, source_id: 'fixed', name: 'Root', action: 'skip' as const, allowed: [] };
  setup({ rows: [fixed] });
  expect(screen.getByText('Skip')).toBeVisible();
  expect(screen.queryByRole('combobox', { name: 'Action for Root' })).not.toBeInTheDocument();
});

it('says that skipping a folder also skips its contents', () => {
  const first = plan.objects.items[0];
  if (first === undefined) throw new Error('Fixture has no item');
  setup({ rows: [{ ...first, kind: 'folder', action: 'skip' }] });
  expect(screen.getByText('and everything in it')).toBeVisible();
});

it('keeps every source person available when a package names many people', async () => {
  const user = userEvent.setup();
  const people = Array.from({ length: 52 }, (_, index) => ({ principal: `person-${index}`, username: `user-${index}`, display_name: `Person ${index}`, matched: 'none' as const, user: null, objects: 1 }));
  setup({ plan: { ...plan, people } });
  await user.click(screen.getByRole('tab', { name: 'People' }));
  expect(screen.queryByText('Person 51')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Next page' }));
  expect(screen.getByText('Person 51')).toBeVisible();
  expect(screen.queryByText('Person 0')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Back to first page' }));
  expect(screen.getByText('Person 0')).toBeVisible();
});

it('keeps an unmatched person as a label without searching for an account', async () => {
  const user = userEvent.setup();
  const handlers = setup({ plan: { ...plan, people: [{ principal: 'source-person', username: 'original', display_name: 'Original author', matched: 'none', user: null, objects: 1 }] } });
  await user.click(screen.getByRole('tab', { name: 'People' }));
  expect(screen.getByText('@original')).toBeVisible();
  await user.click(screen.getByRole('combobox', { name: 'Choose a person for Original author' }));
  await user.click(screen.getByRole('option', { name: 'Imported person' }));
  expect(handlers.onPerson).toHaveBeenCalledExactlyOnceWith('source-person', null);
  expect(calls.people).not.toHaveBeenCalled();
});

it('lets a source person be matched to a visible account', async () => {
  calls.people.mockResolvedValue([person]);
  const user = userEvent.setup();
  const handlers = setup({ plan: { ...plan, people: [{ principal: 'source-person', username: 'original', display_name: 'Original author', matched: 'none', user: null, objects: 1 }] } });
  await user.click(screen.getByRole('tab', { name: 'People' }));
  expect(calls.people).not.toHaveBeenCalled();
  const input = screen.getByRole('combobox', { name: 'Choose a person for Original author' });
  await user.click(input);
  expect(screen.getByRole('option', { name: 'Imported person' })).toBeVisible();
  expect(calls.people).not.toHaveBeenCalled();
  await user.type(input, 'A');
  expect(screen.getByText('Type at least two characters')).toBeVisible();
  await new Promise((resolve) => setTimeout(resolve, 200));
  expect(calls.people).not.toHaveBeenCalled();
  await user.type(input, 'd');
  const account = await screen.findByRole('option', { name: /Ada Lovelace/ });
  await user.click(account);
  expect(handlers.onPerson).toHaveBeenCalledExactlyOnceWith('source-person', person.id);
  expect(calls.people).toHaveBeenCalledExactlyOnceWith('Ad');
});

it('clears a failed account search when the person returns to the imported label', async () => {
  calls.people.mockRejectedValue(new Error('Search failed'));
  const user = userEvent.setup();
  setup({ plan: { ...plan, people: [{ principal: 'source-person', username: 'original', display_name: 'Original author', matched: 'none', user: null, objects: 1 }] } });
  await user.click(screen.getByRole('tab', { name: 'People' }));
  const input = screen.getByRole('combobox', { name: 'Choose a person for Original author' });
  await user.type(input, 'Ad');
  await waitFor(() => expect(input).toHaveAttribute('aria-invalid', 'true'));
  await user.clear(input);
  expect(input).not.toHaveAttribute('aria-invalid');
  expect(screen.getByRole('option', { name: 'Imported person' })).toBeVisible();
});
