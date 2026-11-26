/** The actual Environment Settings entry renders one native row and one true resource source per key. */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SettingsApi from '@/api/settings';
import { EnvironmentSettings } from '@/features/resources/environments/Settings';
import { IdleControl } from '@/features/inspector/IdleControl';
import { localizedSetting } from '@/features/settings/settingText';
import type { Environment, ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailEnvironment } from './resource-detail-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const entries = [
  { key: 'env.lifetime', value: 'temporary' }, { key: 'env.temporary_idle_hours', value: 24 },
  { key: 'network.allowed_hosts', value: ['packages.example.test'] }, { key: 'browser.preset', value: 'headless' },
];

for (const locale of ['en', 'zh-CN'] as const) it(`says that native work keeps the unit awake instead of promising its old idle time (${locale})`, async () => {
  await setLocale(locale);
  const environment = { ...detailEnvironment, status: 'running' as const, idle_stop_at: '2020-01-01T10:00:00Z', idle_stop_deferred: true };
  const view = show(<IdleControl environment={environment} door={{ environmentId: environment.id, threadId: null }} mayManage={false} />);
  expect(screen.getByText(locale === 'en' ? 'Work keeps it awake' : '任务运行中，暂不自动停止')).toBeVisible();
  expect(view.container).not.toHaveTextContent('2020');
});
const server = vi.hoisted(() => ({ values: vi.fn<() => Promise<ResolvedSetting[]>>() }));
vi.mock('@/api/settings', async (load) => {
  const actual = await load<typeof SettingsApi>();
  return { ...actual,
    registryQuery: { queryKey: ['once-settings-registry'], queryFn: () => Promise.resolve(SETTINGS_REGISTRY.filter((def) => entries.some((entry) => entry.key === def.key))), staleTime: Infinity },
    targetSettingsQuery: (target: SettingsApi.SettingsTarget) => ({ queryKey: actual.targetSettingsKey(target), queryFn: server.values, staleTime: Infinity }),
  };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => ({ pathname: '/resources/environments/unit', searchStr: '?tab=settings' }) }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
const priorAppearance = useAppearance.getState();
beforeEach(() => {
  vi.clearAllMocks();
  server.values.mockResolvedValue(entries.map(({ key, value }): ResolvedSetting => ({ key, value, own: { value, revision: 1, locked: false },
    source: { scope: 'environment', node: null }, writable: key !== 'browser.preset', locked_by: null, parts: [] })));
});
afterEach(async () => { cleanup(); useAppearance.setState(priorAppearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps the setting and its resource source once (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const environment: Environment = { ...detailEnvironment, id: 'unit', name: 'Docker workspace', facts: { environment_descriptor: { lifecycle: 'unit' } } };
    const view = show(<EnvironmentSettings environment={environment} />);
    const search = await screen.findByRole('searchbox', { name: i18n.t('settings.editor.search') });
    expect(screen.getAllByRole('searchbox', { name: i18n.t('settings.editor.search') })).toHaveLength(1);
    for (const entry of entries) {
      fireEvent.change(search, { target: { value: entry.key } });
      const definition = SETTINGS_REGISTRY.find((def) => def.key === entry.key);
      if (definition === undefined) throw new Error('the native setting definition must ship');
      const title = localizedSetting(definition, i18n, locale).title;
      await waitFor(() => expect(view.container.querySelectorAll(`[data-setting="${entry.key}"]`)).toHaveLength(1));
      const row = view.container.querySelector<HTMLElement>(`[data-setting="${entry.key}"]`);
      if (row === null) throw new Error('the setting must have its shared row');
      expect(within(row).getByText(title, { exact: true, selector: 'span.font-medium' })).toBeVisible();
      expect(within(row).getByText(i18n.t('settings.compact.setOn', { place: environment.name }), { exact: true })).toBeVisible();
      expect(within(row).queryByText(i18n.t('settings.compact.setOn', { place: i18n.t('unitConfiguration.values') }), { exact: true })).not.toBeInTheDocument();
    }
    expect(server.values).toHaveBeenCalledOnce();
  });
}
