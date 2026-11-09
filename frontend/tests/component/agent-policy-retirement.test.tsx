/** Knowledge and Access read actual shares without turning retired settings into agent permissions. */
import { screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as ShareApi from '@/api/share';
import type * as SettingsApi from '@/api/settings';
import { Agents } from '@/features/access/Agents';
import { ThreadKnowledge, TopicKnowledge } from '@/features/people/Knowledge';
import { i18n } from '@/i18n';
import { ADA, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const retired = vi.hoisted(() => ({ settings: vi.fn() }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({
  queryKey: ['retired-agent-settings', id], queryFn: () => {
    retired.settings(id);
    return Promise.resolve([
      { key: 'sharing.agent_documents', value: 'write' }, { key: 'sharing.agent_reach', value: 'project' },
    ].map((setting) => ({ ...setting, source: { scope: 'default', node: null }, own: null, parts: [], writable: false, locked_by: null })));
  },
}) }));
vi.mock('@/api/share', async (load) => ({ ...await load<typeof ShareApi>(),
  threadSharedQuery: (id: string) => ({ queryKey: ['retirement-thread-shares', id], queryFn: () => Promise.resolve({ policy: 'project', documents_policy: 'write', read_first_paths: [], shares: [] }) }),
  agentSharesQuery: () => ({ queryKey: ['retirement-agent-shares'], queryFn: () => Promise.resolve([]) }),
}));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(), pageQuery: () => ({ queryKey: ['retirement-agent-page'], queryFn: () => Promise.resolve(pageFor('topic')) }) }));
vi.mock('@/api/auth', async (load) => ({ ...await load<object>(), meQuery: { queryKey: ['retirement-reader'], queryFn: () => Promise.resolve(ADA) } }));
vi.mock('@/features/access/KnownToAgents', () => ({ KnownToAgents: () => <p>Explicit contributions remain separate</p> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

it.each(['thread', 'topic'] as const)('shows actual %s knowledge without an obsolete document-policy reader', async (kind) => {
  retired.settings.mockClear();
  show(kind === 'thread' ? <ThreadKnowledge threadId="thread" topicId="topic" /> : <TopicKnowledge topicId="topic" />);
  expect(await screen.findByText(i18n.t('people.noExplicitShares'))).toBeVisible();
  expect(retired.settings).not.toHaveBeenCalled();
  expect(screen.queryByText("Agents here can read, edit and make public documents beside this topic and its main threads.")).not.toBeInTheDocument();
});

it('keeps agent Share available without a policy that grants implicit project reach', async () => {
  retired.settings.mockClear();
  show(<Agents target={{ kind: 'topic', id: 'topic', name: 'Review topic' }} />);
  expect(await screen.findByRole('button', { name: i18n.t('access.agents.share') })).toBeVisible();
  expect(screen.getByText('Explicit contributions remain separate')).toBeVisible();
  expect(retired.settings).not.toHaveBeenCalled();
  expect(screen.queryByText(`The agents of this project can read each other’s threads, by ${i18n.t('access.place.system')}’s setting.`)).not.toBeInTheDocument();
});
