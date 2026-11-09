/** Available servers stay visible beside the actual changes and losses. */
import { expect, it } from 'vitest';

import { changesOf, endsOf, staysOf, type PlanContext } from '@/features/change/plan';
import type { ForkPlan, UserRef } from '@/generated/api-types';
import { i18n } from '@/i18n';

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows a kept MCP server in what stays exactly once (${locale})`, () => {
    const t = i18n.getFixedT(locale);
    const owner: UserRef = { id: 'owner', kind: 'human', username: 'owner', display_name: 'Owner', avatar: { color: 0, text: 'O' } };
    const context: PlanContext = { kind: 'fork', thread: { name: 'Thread', owner, model: 'model', modelDisplay: 'Model', effort: null }, me: owner.id, locale };
    // The line renderers need only these facts; this preview starts no job.
    const plan = {
      result: 'new', from: { provider: { id: 'provider', name: 'Provider' }, agent: { id: 'agent' }, environment: { id: 'env', name: 'Workspace' } },
      to: { provider: { id: 'provider', name: 'Provider' }, agent: { id: 'agent' }, model: 'model', effort: null, predicted: false, environment: { id: 'env', name: 'Workspace' } },
      access: { differs: [] }, history: { mode: 'all', method: 'native_fork', turns: 1, report: { fidelity: 'lossless' } }, files: null, memory: null, mounts: [], leaves: null,
      mcp: [{ id: 'kept-server', name: 'Search', own: false, imported: false, fate: 'kept', reason: null }],
    } as unknown as ForkPlan;
    expect(staysOf(plan, context, t).filter((line) => line.id === 'mcp-kept-server')).toEqual([
      expect.objectContaining({ tone: 'plain', text: t('mcpPlan.kept', { name: 'Search' }) }),
    ]);
    expect([...changesOf(plan, context, t), ...endsOf(plan, context, t)].some((line) => line.id === 'mcp-kept-server')).toBe(false);
  });
}
