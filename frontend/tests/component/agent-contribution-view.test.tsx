/** A contribution's requested ceiling, assessed rights and human management authority are independent. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ShareApi from '@/api/share';
import { AgentRow } from '@/features/access/AgentRow';
import type { AgentShare, DocumentRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, BEA, TOPIC, named, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
const calls = vi.hoisted(() => ({ stop: vi.fn<(id: string) => Promise<void>>() }));
vi.mock('@/api/share', async (load) => ({ ...await load<typeof ShareApi>(), stopAgentShare: calls.stop }));
const material: DocumentRef = { id: 'document', title: 'Shared plan', description: '', description_line: '',
  node: TOPIC, owner: ADA, owners: 1, created_by: ADA, page: 'follows', seq: 1, block_count: 1, bytes: 40, files_bytes: 0,
  version: 1, open_discussions: 0, updated_at: '2026-11-01T00:00:00Z', updated_by: ADA, discussion_level: 'involved', seen_seq: 1, archived: false, deleted: false };
const share: AgentShare = { id: 'contribution', what: material, to: { id: 'recipient', kind: 'thread', name: 'Receiving thread', redacted: false },
  mode: 'live', parts: { documents: 'manage' }, inside: false, entries: [], by: ADA, at: '2026-11-01T00:00:00Z', read_first: false, download: true, snapshot: null, input: null };
const target = { kind: 'document' as const, id: material.id, name: material.title };
beforeEach(() => { calls.stop.mockResolvedValue(undefined); });
afterEach(async () => { vi.clearAllMocks(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps the requested Manage ceiling distinct from this contribution’s current Read (%s)', async (locale) => {
  await setLocale(locale);
  const reduced = { ...share, current: { recipient: 'recipient', parts: { documents: 'read' as const }, state: 'reduced' as const, reason: 'current_limits' as const } };
  show(<ul><AgentRow share={reduced} target={target} page={pageFor('document')} available={{ documents: 'manage' }} meId={ADA.id} /></ul>);
  expect(screen.getByText(i18n.t('agentContributions.requested'))).toBeVisible();
  expect(screen.getByRole('combobox', { name: i18n.t('access.agents.area.documents') })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.manage.name'));
  expect(screen.getByText(i18n.t('agentContributions.state.reduced'))).toBeVisible();
  expect(screen.getByText(i18n.t('agentContributions.reason.current_limits'))).toBeVisible();
});

it('does not present a node-root assessment as its entire subtree’s effective access', () => {
  const perTarget = { ...share, what: TOPIC, inside: true, current: { recipient: 'recipient', parts: null, state: 'unassessed' as const, reason: 'per_target' as const } };
  show(<ul><AgentRow share={perTarget} target={{ kind: 'topic', id: TOPIC.id, name: TOPIC.name ?? '' }} page={pageFor('topic')} available={{ documents: 'manage' }} meId={ADA.id} /></ul>);
  expect(screen.getByText(i18n.t('agentContributions.reason.per_target'))).toBeVisible();
  expect(screen.queryByText(i18n.t('agentContributions.state.active'))).not.toBeInTheDocument();
});

it('retains an unavailable giver’s recorded maximum without inventing active access or ownership', () => {
  const lost: AgentShare = { ...share, by: null, current: { recipient: 'recipient', parts: {}, state: 'inactive', reason: 'giver_unavailable' } };
  const page = pageFor('document'); page.owners_writable = false; page.rows = page.rows.map((row) => ({ ...row, writable: false, can_give: false }));
  show(<ul><AgentRow share={lost} target={target} page={page} available={{}} meId={ADA.id} /></ul>);
  expect(screen.getByText(i18n.t('agentContributions.state.inactive'))).toBeVisible();
  expect(screen.getByText(i18n.t('agentContributions.reason.giver_unavailable'))).toBeVisible();
  expect(screen.getByRole('combobox', { name: i18n.t('access.agents.area.documents') })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.manage.name'));
  expect(screen.getByRole('button', { name: i18n.t('access.agents.stopFor', { name: share.to.name ?? '' }) })).toBeDisabled();
});

it('warns about standing viewer Read and names other recorded contributions before removing only this loan', async () => {
  const entry = named({ kind: 'role', role: 'viewer', at: share.to, count: 1 }, 7);
  const chosen: AgentShare = { ...share, viewer_entries: [entry] };
  const other: AgentShare = { ...share, id: 'another-contribution', by: BEA, parts: { documents: 'read' } };
  show(<ul><AgentRow share={chosen} target={target} page={pageFor('document')} available={{ documents: 'manage' }} meId={ADA.id} remaining={[chosen, other]} /></ul>);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.stopFor', { name: share.to.name ?? '' }) }));
  const dialog = screen.getByRole('alertdialog');
  expect(within(dialog).getByText(i18n.t('agentContributions.viewerStanding'))).toBeVisible();
  expect(within(dialog).getByText(i18n.t('agentContributions.recordedFor', { giver: BEA.display_name, recipient: share.to.name ?? '' }))).toBeVisible();
  expect(calls.stop).not.toHaveBeenCalled();
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('access.agents.stop.confirm') }));
  await waitFor(() => expect(calls.stop).toHaveBeenCalledExactlyOnceWith(share.id));
});

it('retains live revocation through source page management without lendable rights', () => {
  const page = pageFor('document'); page.owners_writable = true;
  show(<ul><AgentRow share={share} target={target} page={page} available={{}} meId={BEA.id} /></ul>);
  expect(screen.getByRole('button', { name: i18n.t('access.agents.stopFor', { name: share.to.name ?? '' }) })).toBeEnabled();
});

it('does not let a lapsed giver revoke solely because their name is on the record', () => {
  const page = pageFor('document'); page.owners_writable = false; page.rows = page.rows.map((row) => ({ ...row, writable: false, can_give: false }));
  show(<ul><AgentRow share={share} target={target} page={page} available={{ documents: 'read' }} meId={ADA.id} /></ul>);
  expect(screen.getByRole('button', { name: i18n.t('access.agents.stopFor', { name: share.to.name ?? '' }) })).toBeDisabled();
});

it('does not let an unrelated giver rewrite another person’s contribution merely through equal lending rights', () => {
  const page = pageFor('document'); page.owners_writable = false; page.rows = page.rows.map((row) => ({ ...row, writable: false }));
  show(<ul><AgentRow share={share} target={target} page={page} available={{ documents: 'manage' }} meId={BEA.id} /></ul>);
  expect(screen.getByRole('button', { name: i18n.t('access.agents.stopFor', { name: share.to.name ?? '' }) })).toBeDisabled();
});
