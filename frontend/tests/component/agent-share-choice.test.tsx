/** The ordinary agent Share form preserves exact area choices and keeps control extras separately authorized. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as ShareApi from '@/api/share';
import { ShareWithAgentsDialog } from '@/features/access/ShareWithAgents';
import { areasOf, partsOfLevel } from '@/features/access/agentShares';
import type { PermissionPage, ShareParts, SharePreview } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const calls = vi.hoisted(() => ({ review: vi.fn<typeof AccessApi.previewShare>(), share: vi.fn<typeof ShareApi.shareWithAgents>() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(), previewShare: calls.review,
  permissionsQuery: { queryKey: ['agent-share-choice-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) } }));
vi.mock('@/api/share', async (load) => ({ ...await load<typeof ShareApi>(), shareWithAgents: calls.share }));
vi.mock('@/features/people/ThreadIdentity', () => ({ ThreadIdentity: () => null }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const target = { kind: 'topic' as const, id: 'topic', name: 'Review topic' };
const preview: SharePreview = { allowed: true, revision: 1, permissions: [], holders: [], effects: { pages: 0, orgs: null, gaining: [], losing: [], more: 0 }, already: [], already_more: 0, agents: [], rows: [], rules: [], refusal: null };
function page(kind: 'topic' | 'document'): PermissionPage {
  const original = pageFor(kind);
  return { ...original, gives: { ...original.gives, agents: ['view', 'edit', 'manage'], agent_parts: partsOfLevel('manage', areasOf(kind, 'inside')) } };
}
function parts(): ShareParts | undefined {
  const request = calls.review.mock.calls.at(-1)?.[1];
  return request?.kind === 'agents' ? request.share.parts ?? undefined : undefined;
}
async function choose(label: string, key: 'none' | 'view' | 'edit' | 'write' | 'manage') {
  await userEvent.click(screen.getByRole('combobox', { name: label }));
  await userEvent.click(await screen.findByRole('option', { name: i18n.t(`choiceCatalog.agentStep.${key}.name`) }));
}
async function customDocuments() {
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
  await choose(i18n.t('access.agents.area.timeline'), 'none');
  await choose(i18n.t('access.agents.area.chat'), 'none');
  await choose(i18n.t('access.agents.area.documents'), 'edit');
  await waitFor(() => expect(parts()).toEqual({ documents: 'edit' }));
}
beforeEach(() => { calls.review.mockReset(); calls.share.mockReset(); calls.review.mockResolvedValue(preview); calls.share.mockResolvedValue({ shares: [] }); });

it('keeps Documents-only Edit as Custom when By area is closed and opened again', async () => {
  show(<ShareWithAgentsDialog target={target} page={page('topic')} open onClose={vi.fn()} initialRecipient={{ id: 'recipient', name: 'Receiving thread' }} />);
  await customDocuments();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
  await waitFor(() => expect(parts()).toEqual({ documents: 'edit' }));
  expect(screen.getByRole('combobox', { name: i18n.t('access.agents.dialog.what') })).toHaveTextContent(i18n.t('choiceCatalog.generalAccess.custom.name'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
  expect(screen.getByRole('combobox', { name: i18n.t('access.agents.area.timeline') })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.none.name'));
  expect(screen.getByRole('combobox', { name: i18n.t('access.agents.area.chat') })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.none.name'));
  expect(calls.share).not.toHaveBeenCalled();
});

it('changes Custom into the full Edit preset only after the person explicitly selects that preset', async () => {
  show(<ShareWithAgentsDialog target={target} page={page('topic')} open onClose={vi.fn()} initialRecipient={{ id: 'recipient', name: 'Receiving thread' }} />);
  await customDocuments();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
  expect(parts()).toEqual({ documents: 'edit' });
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('access.agents.dialog.what') }));
  await userEvent.click(await screen.findByRole('option', { name: i18n.t('choiceCatalog.accessLevel.edit.name') }));
  await waitFor(() => expect(parts()).toEqual({ timeline: 'write', chat: 'write', documents: 'edit' }));
  expect(calls.share).not.toHaveBeenCalled();
});

it('keeps an authorized pure grant available while a separately refused read-first extra disables its submission', async () => {
  calls.review.mockImplementation((_target, request) => Promise.resolve(request.kind === 'agents' && request.share.read_first === true
    ? { ...preview, allowed: false, refusal: { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: '', detail: 'The extra action requires Interact.', request_id: 'synthetic-extra', required: 'thread.interact' } }
    : preview));
  show(<ShareWithAgentsDialog target={{ kind: 'document', id: 'document', name: 'Owned document' }} page={page('document')} open onClose={vi.fn()} initialRecipient={{ id: 'view-only-thread', name: 'Thread I can view' }} />);
  const submit = screen.getByRole('button', { name: i18n.t('access.agents.dialog.button.read', { who: i18n.t('access.agents.dialog.button.ofAgents', { name: 'Thread I can view' }), what: i18n.t('access.agents.dialog.button.documents') }) });
  await waitFor(() => { expect(submit).toBeEnabled(); expect(submit).not.toHaveAttribute('aria-disabled', 'true'); });
  expect(calls.share).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('people.readFirst') }));
  await waitFor(() => expect(submit).toHaveAttribute('aria-disabled', 'true'));
  expect(calls.share).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('people.readFirst') }));
  await waitFor(() => expect(submit).not.toHaveAttribute('aria-disabled', 'true'));
});

it('allows a Documents-only ordinary giver to share without a whole Edit preset or recipient Interact', async () => {
  const base = page('topic');
  const partial = { ...base, rows: base.rows.map((row) => ({ ...row, can_give: row.permission === 'doc.view' || row.permission === 'doc.edit' })),
    gives: { ...base.gives, choices: [], roles: [], agents: [], agent_parts: { documents: 'edit' as const } } };
  calls.review.mockImplementation((_target, request) => Promise.resolve({ ...preview,
    allowed: request.kind === 'agents' && request.share.parts !== undefined && request.share.parts !== null && (request.share.parts.timeline === undefined || request.share.parts.timeline === null) && (request.share.parts.chat === undefined || request.share.parts.chat === null) && request.share.read_first !== true }));
  show(<ShareWithAgentsDialog target={target} page={partial} open onClose={vi.fn()} initialRecipient={{ id: 'view-only-thread', name: 'Thread I can view' }} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
  await choose(i18n.t('access.agents.area.documents'), 'edit');
  await waitFor(() => expect(parts()).toEqual({ documents: 'edit' }));
  const submit = screen.getByRole('button', { name: i18n.t('access.agents.dialog.button.edit', { who: i18n.t('access.agents.dialog.button.ofAgents', { name: 'Thread I can view' }), what: i18n.t('access.agents.dialog.button.documents') }) });
  await waitFor(() => expect(submit).not.toHaveAttribute('aria-disabled', 'true'));
  expect(calls.share).not.toHaveBeenCalled();
  await userEvent.click(submit);
  await waitFor(() => expect(calls.share).toHaveBeenCalledOnce());
  expect(calls.share.mock.calls[0]?.[0]).toMatchObject({ mode: 'live', parts: { documents: 'edit' }, read_first: false });
});

it('keeps ordinary Read for the thread’s viewers unticked until the giver explicitly includes it', async () => {
  show(<ShareWithAgentsDialog target={{ kind: 'document', id: 'document', name: 'Owned document' }} page={page('document')} open onClose={vi.fn()} initialRecipient={{ id: 'recipient', name: 'Receiving thread' }} />);
  const viewers = screen.getByRole('checkbox', { name: i18n.t('agentContributions.viewerChoice') });
  expect(viewers).not.toBeChecked();
  await waitFor(() => expect(calls.review).toHaveBeenCalled());
  const original = calls.review.mock.calls.at(-1)?.[1];
  expect(original?.kind === 'agents' ? original.share : null).not.toHaveProperty('viewers_read');
  expect(calls.share).not.toHaveBeenCalled();
  await userEvent.click(viewers);
  await waitFor(() => {
    const reviewed = calls.review.mock.calls.at(-1)?.[1];
    expect(reviewed?.kind === 'agents' ? reviewed.share : null).toMatchObject({ viewers_read: true, mode: 'live', parts: { documents: 'read' }, read_first: false });
  });
  expect(calls.share).not.toHaveBeenCalled();
});

it('does not carry the viewer Read option into a copy or treat View as permission to deliver it', async () => {
  calls.review.mockImplementation((_target, request) => Promise.resolve(request.kind === 'agents' && request.share.mode === 'snapshot'
    ? { ...preview, allowed: false, refusal: { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: '', detail: 'A copy requires Interact.', request_id: 'copy-interact', required: 'thread.interact' } }
    : preview));
  show(<ShareWithAgentsDialog target={{ kind: 'document', id: 'document', name: 'Owned document' }} page={page('document')} open onClose={vi.fn()} initialRecipient={{ id: 'recipient', name: 'Receiving thread' }} />);
  await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('agentContributions.viewerChoice') }));
  await userEvent.click(screen.getByRole('radio', { name: i18n.t('choiceCatalog.agentShareMode.snapshot.name') }));
  expect(screen.queryByRole('checkbox', { name: i18n.t('agentContributions.viewerChoice') })).not.toBeInTheDocument();
  await waitFor(() => {
    const reviewed = calls.review.mock.calls.at(-1)?.[1];
    expect(reviewed?.kind === 'agents' ? reviewed.share : null).toMatchObject({ mode: 'snapshot' });
    expect(reviewed?.kind === 'agents' ? reviewed.share : null).not.toHaveProperty('viewers_read');
  });
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('access.agents.dialog.button.copyOne', { name: 'Receiving thread' }) })).toHaveAttribute('aria-disabled', 'true'));
  expect(calls.share).not.toHaveBeenCalled();
});


it.each(['en', 'zh-CN'] as const)('keeps repeated Manage choices attached to their own share areas in %s', async (locale) => {
  const previous = currentLocale();
  await setLocale(locale);
  try {
    show(<ShareWithAgentsDialog target={target} page={page('topic')} open onClose={vi.fn()} initialRecipient={{ id: 'recipient', name: 'Receiving thread' }} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
    const group = screen.getByRole('group', { name: i18n.t('access.agents.dialog.byAreaLabel') });
    const rows = within(group).getAllByRole('listitem');
    expect(rows).toHaveLength(3);
    for (const [index, area] of ['timeline', 'chat', 'documents'].entries()) {
      const row = rows[index];
      if (row === undefined) throw new Error('Every share area has its own readable row.');
      const label = i18n.t(`access.agents.area.${area}`);
      await userEvent.click(within(row).getByRole('combobox', { name: label }));
      await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.agentStep.manage.name') }));
      expect(within(row).getByRole('combobox', { name: label })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.manage.name'));
    }
    await waitFor(() => expect(parts()).toMatchObject({ timeline: 'manage', chat: 'manage', documents: 'manage' }));
    expect(calls.share).not.toHaveBeenCalled();
  } finally { await setLocale(previous); }
});
