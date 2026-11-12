/** Native image generation keeps its inherited switch and explains the selected program's capability. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { SettingRow } from '@/features/settings/SettingRow';
import type { KeyDef } from '@/features/settings/settingsModel';
import type { ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
afterEach(async () => { await setLocale('en'); });

function draw(reason: string | null) {
  const def: KeyDef = {
    key: 'agents.image_generation', title: i18n.t('settingRegistry.agents.image_generation.title'),
    description: i18n.t('settingRegistry.agents.image_generation.description'), group: 'agent',
    class: 'parameter', scopes: ['system', 'org', 'project', 'topic', 'thread'], merge: 'replace',
    default: null, example: 'on', schema: { anyOf: [{ type: 'string', enum: ['on', 'off'] }, { type: 'null' }] }, text_key: null, keywords: [],
  };
  const setting: ResolvedSetting = {
    key: def.key, value: null, own: null, writable: false, locked_by: null,
    source: { scope: 'default', node: null }, parts: [], not_applicable_reason: reason,
  };
  show(<ul><SettingRow def={def} setting={setting} target={{ kind: 'node', id: 'thread' }}
    place={{ name: 'Illustrations', kind: 'thread', owner: null, access: { permissions: ['thread.view'] } }} /></ul>);
  return def;
}

it.each(['en', 'zh-CN'] as const)('names the program that does not offer native image generation in %s', async (locale) => {
  await setLocale(locale);
  const def = draw('harness_image_generation_unavailable:Claude Code');
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: def.title }) }));
  expect(await screen.findByText(locale === 'en' ? 'Not offered by Claude Code' : 'Claude Code 不提供此功能')).toBeVisible();
  expect(screen.queryByText(i18n.t('inspector.parameters.subagentsUnsupported'))).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('keeps an unreported program generic without inventing its name in %s', async (locale) => {
  await setLocale(locale);
  const def = draw('harness_image_generation_unavailable');
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: def.title }) }));
  expect(await screen.findByText(locale === 'en' ? 'This agent program has not offered image generation.' : '此代理程序尚未提供图像生成。')).toBeVisible();
  expect(screen.queryByText(/Claude Code/)).not.toBeInTheDocument();
});
