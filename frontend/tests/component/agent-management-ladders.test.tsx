/** Share and thread Access present the same compact agent ladders and independent advanced acts. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as ShareApi from '@/api/share';
import { AgentRow } from '@/features/access/AgentRow';
import { ShareWithAgentsDialog } from '@/features/access/ShareWithAgents';
import { markerSentence } from '@/features/timeline/items/Marker';
import { offeredAgentAccess } from '@/features/start/agentAccessOffer';
import { areasOf, partsOfLevel } from '@/features/access/agentShares';
import type { AgentAccessAvailability, AgentShare, MarkerPayload, ShareParts, SharePreview } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, CATALOGUE, TOPIC, pageFor } from '../unit/general-access-fixtures';
import { show, skipOverflowFallback } from './support';

const calls = vi.hoisted(() => ({ review: vi.fn<typeof AccessApi.previewShare>(), share: vi.fn<typeof ShareApi.shareWithAgents>() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(), previewShare: calls.review,
  permissionsQuery: { queryKey: ['management-ladders-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) } }));
vi.mock('@/api/share', async (load) => ({ ...await load<typeof ShareApi>(), shareWithAgents: calls.share }));
vi.mock('@/features/people/ThreadIdentity', () => ({ ThreadIdentity: () => null }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
skipOverflowFallback();
const target = { kind: 'topic' as const, id: TOPIC.id, name: 'Shared topic' };
const preview: SharePreview = { allowed: true, revision: 1, permissions: [], holders: [], effects: { pages: 0, orgs: null, gaining: [], losing: [], more: 0 }, already: [], already_more: 0, agents: [], rows: [], rules: [], refusal: null };

function parts(): ShareParts | undefined {
  const request = calls.review.mock.calls.at(-1)?.[1];
  return request?.kind === 'agents' ? request.share.parts ?? undefined : undefined;
}
function page() {
  const base = pageFor('topic');
  return { ...base, gives: { ...base.gives, agent_parts: partsOfLevel('manage', areasOf('topic', 'inside')) } };
}
beforeEach(() => { vi.clearAllMocks(); calls.review.mockResolvedValue(preview); calls.share.mockResolvedValue({ shares: [] }); });
afterEach(async () => { await setLocale('en'); });

it('keeps independently available advanced rights when a creation Manage preset is reduced', () => {
  const offer: AgentAccessAvailability = { preset: { where: 'topic', parts: { timeline: 'manage' } }, choices: [
    { where: 'nothing', parts: {} }, { where: 'thread', parts: {} },
    { where: 'topic', parts: { timeline: 'read', thread_fork: true } }, { where: 'project', parts: {} },
  ] };
  const selected = { chosen: null, onChange: vi.fn(), reset: vi.fn() };
  const facts = offeredAgentAccess(selected, { offer, pending: false, retry: vi.fn() });
  expect(facts.value).toEqual({ where: 'topic', parts: { timeline: 'read', thread_fork: true } });
  expect(facts.reduced).toBe(true);
  const onlyArchive = { ...offer, choices: offer.choices.map((choice) => choice.where === 'topic' ? { ...choice, parts: { thread_archive: true } } : choice) };
  expect(offeredAgentAccess(selected, { offer: onlyArchive, pending: false, retry: vi.fn() }).value)
    .toEqual({ where: 'topic', parts: { thread_archive: true } });
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`names an agent fork separately from its responsible giver (${locale})`, async () => {
    await setLocale(locale);
    const payload: MarkerPayload = { marker: 'forked', data: {
      into: { ...TOPIC, kind: 'thread', name: 'Fork result' }, turn_id: 'turn', by: ADA, by_agent: 'acting-agent',
    } };
    expect(markerSentence(payload, i18n.t, { locale })).toBe(i18n.t('timeline.marker.agentForked', { by: 'Ada', into: 'Fork result' }));
    expect(markerSentence(payload, i18n.t, { locale })).not.toBe(i18n.t('timeline.marker.forked', { by: 'Ada', into: 'Fork result' }));
  });

  it(`offers each ladder through Manage and independent Fork/Archive in Share (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<ShareWithAgentsDialog target={target} page={page()} open onClose={vi.fn()} initialRecipient={{ id: 'recipient', name: 'Receiving thread' }} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
    for (const [area, middle] of [['chat', 'post'], ['timeline', 'send']] as const) {
      const control = screen.getByRole('combobox', { name: i18n.t(`access.agents.area.${area}`) });
      await userEvent.click(control);
      expect(screen.getAllByRole('option').map((option) => option.textContent.trim())).toEqual([
        i18n.t('choiceCatalog.agentStep.none.name'), i18n.t('choiceCatalog.agentStep.view.name'),
        i18n.t(`choiceCatalog.agentStep.${middle}.name`), i18n.t('choiceCatalog.agentStep.manage.name'),
      ]);
      await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.agentStep.manage.name') }));
      await waitFor(() => expect(parts()?.[area]).toBe('manage'));
    }
    expect(view.container.querySelector('[data-choice-selected-description]')).toBeNull();
    await userEvent.click(screen.getByText(i18n.t('access.advanced.title')));
    expect(screen.getByRole('checkbox', { name: i18n.t('access.agents.fork') })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: i18n.t('access.agents.archive') })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: i18n.t('access.agents.fork') })).toBeDisabled();
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('access.agents.area.timeline') }));
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.agentStep.view.name') }));
    await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('access.agents.fork') }));
    await waitFor(() => expect(parts()).toMatchObject({ timeline: 'read', chat: 'manage', thread_fork: true }));
    expect(parts()?.thread_archive).toBeUndefined();
    expect(calls.share).not.toHaveBeenCalled();
  });

  it(`keeps requested Manage and exact advanced rows readable in a thread Access contribution (${locale})`, async () => {
    await setLocale(locale);
    const share: AgentShare = { id: 'contribution', what: TOPIC,
      to: { id: 'recipient', kind: 'thread', name: 'Receiving thread', redacted: false }, mode: 'live',
      parts: { timeline: 'read', chat: 'manage', thread_fork: true, thread_archive: true }, inside: true,
      entries: [], by: ADA, at: '2026-11-01T00:00:00Z', read_first: false, download: true, snapshot: null, input: null,
      current: { recipient: 'recipient', parts: null, state: 'unassessed', reason: 'per_target' } };
    const view = show(<ul><AgentRow share={share} target={target} page={page()} available={partsOfLevel('manage', areasOf('topic', 'inside'))} meId={ADA.id} /></ul>);
    expect(screen.getByRole('combobox', { name: i18n.t('access.agents.area.chat') })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.manage.name'));
    await userEvent.click(screen.getByText(i18n.t('access.advanced.title')));
    expect(screen.getByRole('checkbox', { name: i18n.t('access.agents.fork') })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: i18n.t('access.agents.archive') })).toBeChecked();
    expect(screen.getByText(i18n.t('agentContributions.reason.per_target'))).toBeVisible();
    expect(view.container.querySelector('[data-choice-selected-description]')).toBeNull();
    expect(view.container).not.toHaveTextContent(i18n.t('choiceCatalog.agentStep.manage.description'));
  });

  it(`disables unowned management and Archive while leaving an independent Fork grant available (${locale})`, async () => {
    await setLocale(locale);
    const base = page();
    const limited = { ...base, gives: { ...base.gives, agent_parts: { timeline: 'read' as const, thread_fork: true } } };
    show(<ShareWithAgentsDialog target={target} page={limited} open onClose={vi.fn()} initialRecipient={{ id: 'recipient', name: 'Receiving thread' }} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('access.agents.area.timeline') }));
    expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.agentStep.manage.name') })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.keyboard('{Escape}');
    await userEvent.click(screen.getByText(i18n.t('access.advanced.title')));
    expect(screen.getByRole('checkbox', { name: i18n.t('access.agents.archive') })).toBeDisabled();
    const fork = screen.getByRole('checkbox', { name: i18n.t('access.agents.fork') });
    expect(fork).toBeEnabled();
    await userEvent.click(fork);
    await waitFor(() => expect(parts()).toEqual({ timeline: 'read', thread_fork: true }));
  });
}
