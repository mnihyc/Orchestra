/** Typed program sources retain every real change line without native stand-ins. */
import { cleanup } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { PlanLists } from '@/features/change/Lists';
import { changesOf, endsOf, sourceEnvironment, staysOf, type PlanContext } from '@/features/change/plan';
import { markerSentence } from '@/features/timeline/items/Marker';
import type { ForkPlan, MarkerPayload } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { resourcePlan } from './plan-resource-fixtures';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`renders a builtin source and its native destination without inventing a source profile or workspace (${locale})`, async () => {
  // EN browser: kept read-only thread > Switch provider or agent > choose real
  // native destination > review Program, model and environment > Cancel.
  // 中文浏览器：已保留的只读会话 > 切换提供方或智能体 > 选择真实原生目标 >
  // 查看程序、模型及环境 > 取消。不存在的来源配置和环境不得出现在方案或历史中。
  await setLocale(locale);
  const base = resourcePlan('work');
  if ('new' in base.to.environment) throw new Error('The fixture uses its existing native destination');
  const targetEnvironment = {...base.to.environment, name: 'Native workspace'};
  const plan: ForkPlan = { ...base, mounts: [], mcp: [],
    from: { kind: 'program', program: { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false }, provider: base.from.provider, model: 'old-model' },
    to: { ...base.to, environment: targetEnvironment } };
  const context: PlanContext = {kind: 'switch', thread: {name: 'Work', owner: base.owner.after, model: null, modelDisplay: null, effort: null}, me: null, locale,
    models: [{model_name: 'old-model', display_name: 'Earlier API model'}, {model_name: base.to.model, display_name: 'Native model'}]};
  const t = i18n.t.bind(i18n);
  const changes = changesOf(plan, context, t);
  const program = changes.find((line) => line.id === 'program');
  expect(program?.text).toBe(t('change.plan.program', {from: t('ask.threadProgram'), to: plan.to.agent.name}));
  expect(program?.resources).toHaveLength(1);
  expect(changes.some((line) => line.id === 'agent')).toBe(false);
  expect(changes.find((line) => line.id === 'model')?.text).toContain('Earlier API model');
  expect(changes.find((line) => line.id === 'environment')?.text).toBe(t('change.plan.startsIn', {environment: 'Native workspace'}));
  expect(sourceEnvironment(plan)).toBeNull();
  const stays = staysOf(plan, context, t);
  expect(stays.some((line) => line.id === 'same-environment')).toBe(false);
  const view = show(<PlanLists changes={changes} stays={stays} ends={endsOf(plan, context, t)} />);
  expect(view.container).toHaveTextContent(t('ask.threadProgram'));
  expect(view.container).toHaveTextContent('Native workspace');
  const marker: MarkerPayload = { marker: 'switched', data: { by: base.owner.after,
    from: {...plan.from, directory: ''}, to: {agent: plan.to.agent, provider: plan.to.provider, environment: targetEnvironment, directory: '/work'}, report: plan.history.report } };
  expect(markerSentence(marker, t, {locale})).toContain(plan.to.agent.name);
  expect('agent' in marker.data.from).toBe(false);
  expect('environment' in marker.data.from).toBe(false);
});

it('keeps native agent source facts and same-environment lines intact', () => {
  const plan = resourcePlan('native');
  const context: PlanContext = {kind: 'switch', thread: {name: 'Native', owner: plan.owner.after, model: plan.to.model, modelDisplay: 'Model', effort: null}, me: null, locale: 'en'};
  expect(sourceEnvironment(plan)).not.toBeNull();
  expect(changesOf(plan, context, i18n.t.bind(i18n)).some((line) => line.id === 'program')).toBe(false);
  expect(staysOf(plan, context, i18n.t.bind(i18n)).some((line) => line.id === 'same-environment')).toBe(true);
});
