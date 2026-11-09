/** Recorded destinations, rather than the unchanged provider, describe an environment switch. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { changeJobWords } from '@/features/change/jobWords';
import type { PlanContext } from '@/features/change/plan';
import { JobProgress } from '@/features/change/Progress';
import { Result } from '@/features/change/Result';
import { MarkerLine, markerSentence } from '@/features/timeline/items/Marker';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { Job, MarkerPayload, PlaceEnvironment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { creationHost, placeEnvironment } from '../unit/start-fixtures';
import { marker } from '../doubles/thread-rows';
import { resourcePlan } from './plan-resource-fixtures';
import { show } from './support';

const context: TimelineContext = { threadId: 'work', threadName: 'Work', detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: false, live: true, lit: null, selection: NOT_PICKING };
const plan = resourcePlan('work');
if (plan.from.kind !== 'agent' || plan.from.environment === null) throw new Error('The environment-switch fixture needs its actual source workspace');
const destination: PlaceEnvironment = placeEnvironment('destination', {
  environment: { ...plan.from.environment, id: 'destination', name: 'Main' },
  host: { id: 'host-b', name: 'Build host', address: 'org.build' }, facts: { cpus: 4, memory_mb: 4096, os: 'linux' },
});
const moved: MarkerPayload = { marker: 'environment_moved', data: { by: plan.owner.after,
  from: { ...destination.environment, id: 'source' }, to: destination.environment,
  from_host: { id: 'host-a', name: 'Desk host', address: 'org.desk' }, to_host: destination.host,
  from_machine: null, to_machine: null, mounts_detached: ['Research data'], path_map: { from: '/old', to: '/new' } } };
const nativeJob: Job = { id: 'switch-job', kind: 'thread_fork', state: 'running', node_id: 'work', environment_id: 'source', provider_id: null,
  batch_id: null, created_by: plan.owner.after, created_at: '2026-11-01T12:00:00Z', started_at: '2026-11-01T12:00:00Z', finished_at: null,
  error: null, log: null, result: null, progress: { phase: 'after_up', percent: 70, phases: [{ name: 'after_up', state: 'running', weight: 1 }],
    message: null, bytes_done: 0, bytes_total: null, files_done: 0, files_total: null, rate_bps: null, eta_seconds: null } };
const planContext: PlanContext = { kind: 'switch', thread: { name: 'Work', model: 'm1', modelDisplay: 'Model', effort: null, owner: plan.owner.after }, me: null, locale: 'en' };

afterEach(async () => { cleanup(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) for (const width of [400, 1440]) {
  it(`names the environment and recorded host throughout progress, completion and history (${locale}/${theme}/${width})`, async () => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
    const accepted = { ...plan, to: { ...plan.to, environment: destination.environment } };
    const words = changeJobWords(accepted, false, true, i18n.t.bind(i18n), planContext, [destination]);
    const heading = locale === 'en' ? 'Switching to Main on Build host' : '正在切换到 Build host 上的 Main';
    const done = locale === 'en' ? 'This thread now runs in Main.' : '此线程现在在 Main 中运行。';
    const view = show(<div style={{ width }}><JobProgress job={nativeJob} heading={words.heading} reconnecting={false} /></div>);
    expect(screen.getByRole('heading', { name: heading })).toBeVisible();
    expect(screen.getByText(locale === 'en' ? 'Running after-start commands' : '正在运行启动后的命令')).toBeVisible();
    expect(view.container).not.toHaveTextContent(plan.to.provider.name);
    const item = marker('moved', moved);
    if (item.kind !== 'marker') throw new Error('The history row must be a marker');
    view.rerender(<div style={{ width }}><Result job={{ ...nativeJob, state: 'succeeded' }} done={words.done} stoppedTurn={false} waited={0} subject="Work" />
      <TimelineContextValue.Provider value={context}><MarkerLine item={item} /></TimelineContextValue.Provider></div>);
    expect(screen.getByText(done)).toBeVisible();
    const history = locale === 'en' ? 'Ada Lovelace moved Main on Desk host to Main on Build host · Detached mounts: Research data'
      : 'Ada Lovelace 将 Desk host 上的 Main 迁移到 Build host 上的 Main · 已卸载挂载：Research data';
    expect(screen.getByText(history)).toBeVisible();
    expect(view.container).not.toHaveTextContent(plan.to.provider.name);
  });
}
for (const locale of ['en', 'zh-CN'] as const) it(`never invents location facts in older history (${locale})`, async () => {
  await setLocale(locale);
  if (moved.marker !== 'environment_moved') throw new Error('Expected the environment move marker');
  const old: MarkerPayload = { marker: 'environment_moved', data: { ...moved.data, from_host: null, to_host: null } };
  const text = markerSentence(old, i18n.t.bind(i18n), { locale });
  expect(text).toContain(locale === 'en' ? 'Main (location not recorded)' : 'Main（未记录位置）');
  expect(text).not.toContain('Desk host'); expect(text).not.toContain('Build host');
  const provider = changeJobWords(plan, false, false, i18n.t.bind(i18n), planContext, []);
  expect(provider.heading).toContain(plan.to.provider.name);
  expect(provider.done).toContain(plan.to.provider.name);
});

for (const locale of ['en', 'zh-CN'] as const) it(`uses the admitted new place and never substitutes a provider for an unavailable location (${locale})`, async () => {
  await setLocale(locale);
  const t = i18n.t.bind(i18n);
  const existing = { ...plan, to: { ...plan.to, environment: destination.environment } };
  // No cached place is a missing location fact, not permission to guess a machine.
  const unknown = changeJobWords(existing, false, true, t, planContext, []);
  const view = show(<JobProgress job={nativeJob} heading={unknown.heading} reconnecting={false} />);
  expect(screen.getByRole('heading', { name: locale === 'en' ? 'Switching to Main' : '正在切换到 Main' })).toBeVisible();
  expect(view.container).not.toHaveTextContent(plan.to.provider.name);
  const fresh = { ...plan, to: { ...plan.to, environment: { new: { name: 'New work', place: creationHost() } } } };
  const known = changeJobWords(fresh, false, true, t, planContext, []);
  view.rerender(<JobProgress job={nativeJob} heading={known.heading} reconnecting={false} />);
  expect(screen.getByRole('heading', { name: locale === 'en' ? 'Switching to New work on Build host' : '正在切换到 Build host 上的 New work' })).toBeVisible();
});
