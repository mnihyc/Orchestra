/** UA-21: one display-name path for pickers, plan differences and completion; routing stays on the wire. */
import { afterEach, expect, it } from 'vitest';

import { initialChoices, requestOf } from '@/features/change/choices';
import { effortLabel, modelLabel } from '@/features/change/modelLabel';
import { changesOf, targetModelName, targetModelWords, type PlanContext } from '@/features/change/plan';
import type { ForkPlan, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`names the chosen model consistently without exposing its routing key (${locale})`, async () => {
  await setLocale(locale);
  const t = i18n.getFixedT(locale);
  const models = [{ model_name: 'private_route_to', display_name: 'Readable Model' }];
  const owner: UserRef = { id: 'owner', kind: 'human', username: 'owner', display_name: 'Owner', avatar: { color: 0, text: 'O' } };
  const context: PlanContext = { kind: 'switch', thread: { name: 'Thread', owner, model: 'private_route_from', modelDisplay: 'Earlier Model', effort: null }, me: 'owner', locale, models };
  // Only the facts turned into lines by changesOf are needed; this fixture starts no job.
  const plan = {
    result: 'replace', from: { provider: { id: 'provider', name: 'Provider' }, agent: { id: 'agent' }, environment: { id: 'environment', name: 'Workspace' } },
    to: { provider: { id: 'provider', name: 'Provider' }, agent: { id: 'agent' }, model: 'private_route_to', effort: 'high', predicted: false, environment: { id: 'environment', name: 'Workspace' } },
    history: { mode: 'all', method: 'native_fork', turns: 2, report: { fidelity: 'lossless' } }, files: null, memory: null, mounts: [], mcp: [],
  } as unknown as ForkPlan;
  const words = `Readable Model (${t('choiceCatalog.effort.high.name')})`;
  expect(modelLabel('private_route_to', models, null, t)).toBe('Readable Model');
  expect(targetModelName(plan, context, t)).toBe('Readable Model');
  expect(targetModelWords(plan, context, t)).toBe(words);
  const change = changesOf(plan, context, t).find((line) => line.id === 'model');
  expect(change?.text).toBe(t('changeCompact.modelChange', { from: 'Earlier Model', to: words }));
  expect(change?.text).not.toContain('private_route');
  expect(modelLabel('private_route_missing', [], 'private_route_missing', t)).toBe(t('changeCompact.unnamedModel'));
  expect(modelLabel('private_route_current', [], 'Current display name', t)).toBe('Current display name');
  expect(effortLabel('private_effort', t)).toBe(t('changeCompact.otherEffort'));
  expect(effortLabel(null, t)).toBeNull();
  expect(requestOf({ ...initialChoices('replace'), providerId: 'target-provider', model: 'private_route_to' })).toMatchObject({ to: { model: 'private_route_to' } });
});
