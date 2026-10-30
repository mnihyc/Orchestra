/** Search settings open the relevant translated guide without changing the setting's source or value. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { SettingHint } from '@/features/help/SettingHint';
import { SettingEntry } from '@/features/help/SettingEntry';
import { GuideSettings } from '@/features/help/GuideSettings';
import { useHelpState } from '@/features/help/state';
import { hintOf } from '@/features/help/hints';
import { localizedSetting } from '@/features/settings/settingText';
import { SettingRow } from '@/features/settings/SettingRow';
import { withGroup } from '@/features/settings/settingsModel';
import { settingArticle } from '@/features/help/settings';
import type { ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const open = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => open }));
vi.mock('@/features/workbench/Narrow', () => ({ useShape: () => 'wide' }));

beforeEach(() => { open.mockClear(); useHelpState.setState({ guideContext: null, settingContext: null }); });
afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('explains unrecorded file types and links to the filter guide in %s', async (locale) => {
  await setLocale(locale);
  const hint = hintOf('search-file-type', i18n.t);
  expect(hint.what).toBe(locale === 'en' ? 'Some conversation files have no recorded type. Choose Any to include them.' : '部分对话文件没有记录类型。选择“任意”可包含它们。');
  expect(hint.article).toEqual({id:'finding-things', heading:'filters-and-words-only'});
});

for (const locale of ['en', 'zh-CN'] as const) it.each([
  ['search.semantic.enabled', 'turn-on-semantic-search'],
  ['search.semantic.embeddings', 'choose-embeddings'],
  ['search.semantic.max_disk_share', 'disk-limit'],
  ['search.semantic.tuning', 'indexing-and-ranking'],
  ['search.word_tuning', 'indexing-and-ranking'],
])(`opens %s at its semantic guide section in ${locale}`, async (key, heading) => {
  await setLocale(locale);
  const source = SETTINGS_REGISTRY.find((entry) => entry.key === key);
  if (source === undefined) throw new Error('the search setting must be shipped');
  const def = localizedSetting(source, i18n);
  const target = { kind: 'node' as const, id: 'org-1' };
  const user = userEvent.setup();
  show(<SettingHint def={def} target={target} />);
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: def.title }) }));
  await user.click(screen.getByRole('button', { name: i18n.t('helpGroups.readGuide', { guide: i18n.t('help.hints.semantic-search-index.title') }) }));
  expect(open).toHaveBeenCalledExactlyOnceWith('semantic-search', heading);
  expect(useHelpState.getState().guideContext).toEqual({ article: 'semantic-search', target });
  expect(useHelpState.getState().settingContext).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('keeps all semantic setting entries reachable from the guide in %s', async (locale) => {
  await setLocale(locale);
  const next = vi.fn();
  show(<GuideSettings article="semantic-search" onOpen={next} />);
  const title = i18n.t('settingRegistry.search.semantic.embeddings.title');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: title }) }));
  expect(next).toHaveBeenCalledExactlyOnceWith('search.semantic.embeddings');
  expect(screen.getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: i18n.t('settingRegistry.search.word_tuning.title') }) })).toBeVisible();
  expect(screen.getAllByRole('button')).toHaveLength(5);
});

it.each(['en', 'zh-CN'] as const)('opens the full word-retrieval entry from its read-only settings row in %s', async (locale) => {
  await setLocale(locale);
  const source = SETTINGS_REGISTRY.find((entry) => entry.key === 'search.word_tuning');
  if (source === undefined) throw new Error('word retrieval must be shipped');
  const def = localizedSetting(source, i18n);
  const target = { kind: 'system' as const };
  const setting: ResolvedSetting = { key: def.key, value: def.default, own: null, locked_by: null,
    source: { scope: 'default', node: null }, parts: [], writable: false };
  show(<SettingRow def={withGroup(def)} setting={setting} target={target}
    place={{ name: 'System', owner: null, access: null, kind: null }} />);
  expect(screen.queryByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) })).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: def.title }) }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: def.title }) }));
  expect(open).toHaveBeenCalledExactlyOnceWith(settingArticle(def.key));
  expect(useHelpState.getState().settingContext).toEqual({ key: def.key, target });
});

it('leads a setting entry back to its setup guide', async () => {
  const def = SETTINGS_REGISTRY.find((entry) => entry.key === 'search.semantic.enabled');
  if (def === undefined) throw new Error('the search setting must be shipped');
  show(<SettingEntry def={def} onBack={() => undefined} />);
  await userEvent.click(screen.getByRole('button', { name: 'Semantic search' }));
  expect(open).toHaveBeenCalledExactlyOnceWith('semantic-search', 'turn-on-semantic-search');
});

for (const locale of ['en', 'zh-CN'] as const) it.each([
  ['search.word_tuning', ['searchTuning.fields.scopeRows', 'searchTuning.fields.fieldLimits', 'searchTuning.fields.title', 'searchTuning.fields.description', 'searchTuning.fields.heading', 'searchTuning.fields.body']],
  ['search.semantic.tuning', ['searchTuning.fields.graphBreadth', 'searchTuning.fields.graphScan', 'admin.semantic.fields.exactFilter']],
] as const)(`names the structured fields of %s consistently in its full entry in ${locale}`, async (key, labels) => {
  await setLocale(locale);
  const source = SETTINGS_REGISTRY.find((entry) => entry.key === key);
  if (source === undefined) throw new Error('the search setting must be shipped');
  const def = localizedSetting(source, i18n);
  show(<SettingEntry def={def} onBack={() => undefined} />);
  const section = screen.getByRole('heading', { name: i18n.t('helpRelease.values') }).closest('section');
  if (section === null) throw new Error('schema values have their own section');
  for (const label of labels) {
    const name = i18n.t(label);
    expect(within(section).getByText((text) => text === name || text === `${name} (${i18n.t('helpRelease.required')})`)).toBeVisible();
  }
  expect(section).not.toHaveTextContent('scope candidates');
  expect(section).not.toHaveTextContent('ef search');
});
