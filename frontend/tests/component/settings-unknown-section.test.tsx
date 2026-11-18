/** Invalid settings addresses name their missing section and retain the real section list. */
import { screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as NodesApi from '@/api/nodes';
import { OrgSettings } from '@/features/settings/org/OrgSettings';
import { ProjectSettings, TopicScopeSettings } from '@/features/settings/ScopeSettings';
import { settingsNavigation } from '@/features/settings/navigation';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

vi.mock('@/shell/org', () => ({ selectOrg: vi.fn() }));
vi.mock('@/features/settings/org/General', () => ({ General: () => <p data-testid="general-form">General form</p> }));
vi.mock('@/features/settings/org/Policies', () => ({ Policies: () => <p data-testid="defaults-form">Defaults form</p> }));
const source = vi.hoisted(() => ({ world: null as World | null }));
function scene() {
  const world = source.world; if (world === null) throw new Error('The native settings fixture is initialized');
  const person = world.people.get('ada'); const org = world.places.rowOf(ACME.org), project = world.places.rowOf(ACME.web), topic = world.places.rowOf(ACME.fixLogin);
  if (person === undefined || org === undefined || project === undefined || topic === undefined) throw new Error('The fixture has readable settings places');
  return { org: world.places.org(person, org), project: world.places.project(person, project), topic: world.places.topic(person, topic) };
}
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  orgBySlugQuery: (slug: string) => ({ queryKey: ['unknown-org', slug], queryFn: () => Promise.resolve(scene().org) }),
  projectQuery: (id: string) => ({ queryKey: ['unknown-project', id], queryFn: () => Promise.resolve(scene().project) }),
}));
beforeEach(() => { source.world = new World(); });
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['org', 'project', 'topic'] as const) for (const section of kind === 'org' ? ['agents'] : ['agents', 'members']) it(`keeps an unknown ${kind}/${section} section distinct from General and links Defaults by its actual name (${locale})`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale); const data = scene();
  const view = kind === 'org' ? <OrgSettings slug="acme" section={section} /> : kind === 'project' ? <ProjectSettings projectId={ACME.web} section={section} /> : <TopicScopeSettings topic={data.topic} section={section} onSection={vi.fn()} />;
  show(view);
  expect(await screen.findByRole('heading', { name: t('settings.sectionUnavailable') })).toBeVisible();
  expect(screen.getByText(t('settings.chooseSection'))).toBeVisible(); expect(screen.queryByTestId('general-form')).not.toBeInTheDocument();
  const name = kind === 'org' ? data.org.name : kind === 'project' ? data.project.name : data.topic.name;
  const nav = screen.getByRole('navigation', { name: t('settings.org.sections', { name }) });
  expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(0);
  expect(within(nav).queryByRole('combobox')).not.toBeInTheDocument();
  const link = within(nav).getByRole('link', { name: t('day100.names.defaults') });
  expect(link).toHaveAttribute('href', kind === 'org' ? '/org/acme/settings/defaults' : kind === 'project' ? `/project/${ACME.web}/settings?section=defaults` : `/topic/${ACME.fixLogin}?tab=settings&section=defaults`);
});
it.each(['en', 'zh-CN'] as const)('never treats an unrecognized explicit section as the implicit default (%s)', async (locale) => {
  await setLocale(locale);
  expect(settingsNavigation('/org/acme/settings/agents')).toMatchObject({ kind: 'org', current: '' });
  expect(settingsNavigation('/project/web/settings', { section: 'agents', setting: 'agent.model' })).toMatchObject({ current: '' });
  expect(settingsNavigation('/topic/topic', { tab: 'settings', section: 'agents', setting: 'agent.model' })).toMatchObject({ current: '' });
  const view = show(<OrgSettings slug="acme" section="policies" />);
  expect(await screen.findByText(i18n.t('settings.chooseSection'))).toBeVisible(); expect(screen.queryByTestId('defaults-form')).not.toBeInTheDocument();
  view.unmount(); show(<OrgSettings slug="acme" section="defaults" />);
  expect(await screen.findByTestId('defaults-form')).toBeVisible();
});
