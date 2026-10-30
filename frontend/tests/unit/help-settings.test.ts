// @vitest-environment node
/** The example and exact setting/source are what the reader finds, including hidden-source refusal. */
import { describe, expect, it } from 'vitest';
import { authoredSettingGuide, findSettingHelp, settingArticle, settingDefinitionForKey, settingKeyOf } from '@/features/help/settings';
import { sourceSettingHref } from '@/features/help/WhereSet';
import { readHelpState } from '@/features/help/Dismissed';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { helpEntries, answer } from '@/features/finder/local';
import { localizedGuide } from '@/help/localized';
import { guide } from '@/help/library';
import { GUIDANCE } from '@/help/guidance';
import { boundedStep, nextStep } from '@/help/tutorials/progress';
import type { SettingSource } from '@/generated/api-types';

describe('settings and the decision guides', () => {
  it('keeps the authored example beside its default for every registry entry', () => {
    expect(SETTINGS_REGISTRY.length).toBeGreaterThanOrEqual(146);
    for (const def of SETTINGS_REGISTRY) {
      expect(Object.hasOwn(def, 'example'), def.key).toBe(true);
      expect(settingKeyOf(settingArticle(def.key))).toBe(def.key);
    }
    for (const guide of GUIDANCE) for (const key of guide.settings) expect(SETTINGS_REGISTRY.some((def) => def.key === key), key).toBe(true);
    expect(findSettingHelp(SETTINGS_REGISTRY, 'requests.expiry_hours').map((def) => def.key)).toContain('requests.expiry_hours');
    expect(settingDefinitionForKey(SETTINGS_REGISTRY, 'agent.params.service_tier')?.key).toBe('agent.params');
  });
  it('finds the approval guide by a how-do-I phrase, a technical key and the translated setting name', () => {
    const localized = localizedGuide(guide, (key) => key, [{ key: 'approval.mode', title: '审批方式', keywords: [] }]);
    const entries = helpEntries(localized.articles);
    for (const query of ['how do I approve', 'approval.mode', '审批方式']) expect(answer(entries, query).some(({ item }) => item.article.id === 'how-approvals-work'), query).toBe(true);
  });
  it('leads every search setting to an existing section in both authored languages', () => {
    for (const def of SETTINGS_REGISTRY.filter((entry) => entry.key.startsWith('search.'))) {
      const link = authoredSettingGuide(def.key);
      expect(link, def.key).not.toBeNull();
      const article = guide.articles.find((entry) => entry.id === link?.article);
      expect(article?.headings.map((heading) => heading.id), def.key).toContain(link?.heading);
      expect(article?.translations?.['zh-CN']?.headings.map((heading) => heading.id), def.key).toContain(link?.heading);
    }
  });
  it('links to the exact key at its visible source and makes no hidden-place link', () => {
    const source: SettingSource = { scope: 'project', node: { id: 'project-1', kind: 'project', name: 'Web', redacted: false } };
    const def = { key: 'requests.expiry_hours', class: 'policy' as const };
    expect(sourceSettingHref(source, def)).toBe('/project/project-1/settings?section=policies&setting=requests.expiry_hours');
    expect(sourceSettingHref({ ...source, node: { ...source.node!, redacted: true, name: null } }, def)).toBeNull();
    expect(sourceSettingHref({ scope: 'system', node: null }, def)).toBe('/admin/settings?setting=requests.expiry_hours');
    expect(sourceSettingHref({ scope: 'org', node: { ...source.node!, kind: 'org' } }, def)).toBeNull();
    expect(sourceSettingHref({ scope: 'org', node: { ...source.node!, kind: 'org' } }, def, undefined, 'team/web')).toBe('/org/team%2Fweb/settings/policies?setting=requests.expiry_hours');
    expect(sourceSettingHref({ scope: 'provider_own', node: null }, def, { kind: 'provider', id: 'provider-1' })).toBe('/resources/providers/provider-1?tab=settings&setting=requests.expiry_hours');
  });
});

describe('leaving and resuming a tutorial', () => {
  it('retains another help field and accepts only finite whole progress', () => {
    expect(readHelpState({ closed: ['tip'], finished: ['first-thread'], other: 'keep', progress: { ports: 2, negative: -1, fractional: 1.5, invalid: '2' } })).toEqual({ closed: ['tip'], finished: ['first-thread'], other: 'keep', progress: { ports: 2 } });
  });
  it('holds a missing target and counts completion only on the visible last step', () => {
    expect(nextStep(2, 3, false)).toBe(2);
    expect(nextStep(2, 3, true)).toBe('finished');
    expect(nextStep(0, 3, true)).toBe(1);
    expect(boundedStep(999, 3)).toBe(2);
    expect(boundedStep(Number.NaN, 3)).toBe(0);
  });
});
