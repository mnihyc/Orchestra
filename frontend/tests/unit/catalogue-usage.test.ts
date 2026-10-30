// @vitest-environment node
/** Ordinary catalogues describe the working product, and every retained message has a product reader. */
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import type { SettingDef } from '@/generated/api-types';
import { catalogueReaders, messages, productSources, UNFINISHED_PRODUCT, unusedMessages, type RuntimeMessages } from './catalogue-usage';

/** The source-audit suites share this 20-second ceiling; both languages read one source snapshot. */
const SOURCE_AUDIT_MS = 20_000;

/** Opaque runtime keys are declared beside the gate, with their actual producer and finite vocabulary.
 * Translated literal templates declare their fixed prefix/suffix directly in source; local
 * bindings are followed, never unrelated filename/cache templates. No namespace is exempted.
 * Removing a producer removes its declaration. */
const RUNTIME_MESSAGES: readonly RuntimeMessages[] = [
  { source: 'features/admin/Charts.tsx', reason: 'Chart label callbacks receive the existing Translate adapter and these finite native series labels', keys: [
    ...['completed', 'failed', 'interrupted'].map((name) => `admin.overview.results.${name}`),
    ...['cap', 'lock'].map((name) => `admin.overview.waits.${name}`),
    ...['pinned', 'cache', 'free'].map((name) => `admin.overview.room.${name}`),
  ] },
  { source: 'features/settings/settingText.ts', reason: 'The registered settings supply their metadata keys; this reader probes the canonical, declared and compatibility vocabularies in both languages', keys: SETTINGS_REGISTRY.flatMap((definition: Pick<SettingDef, 'key' | 'text_key'>) => [
    `settingRegistry.${definition.key}`, ...(definition.text_key === null ? [] : [definition.text_key]), `day100.builtin.${definition.key}`,
  ].flatMap((prefix) => ['title', 'description', 'keywords'].map((field) => `${prefix}.${field}`))) },
  { source: 'features/workbench/containers.ts', reason: 'Built-in container registration supplies title/about keys; navigation/resources/help use their explicit shared aliases', keys: [
    'conversation', 'team', 'code', 'environment', 'agent', 'document',
  ].flatMap((name) => ['title', 'about'].map((field) => `workbench.containers.${name}.${field}`)) },
  { source: 'features/workbench/choiceWords.ts', reason: 'The two finite registries supply a late-bound name/description pair to viewChoice/containerChoice', keys: [
    ...['screen', 'navigation', 'resource-navigation', 'outline', 'history', 'chat', 'documents', 'people', 'changes', 'files', 'git', 'environment', 'processes', 'resources', 'ports', 'browsers', 'displays', 'parameters', 'usage', 'knowledge', 'given', 'diagnostics', 'terminal', 'comments', 'links', 'help', 'browser', 'vnc', 'file', 'difference', 'search-result'].flatMap((name) => ['name', 'description'].map((field) => `workbench.choices.views.${name}.${field}`)),
    ...['navigation', 'resource-navigation', 'conversation', 'team', 'code', 'environment', 'agent', 'document', 'help'].flatMap((name) => ['name', 'description'].map((field) => `workbench.choices.containers.${name}.${field}`)),
  ] },
  { source: 'features/resources/ResourceNames.tsx', reason: 'MCP/mount list prefixes selected by kind, followed by the five literal property names in that shared view', keys: [
    'resourceDetails.mcp.list.lead', 'resourceDetails.mcp.list.search', 'resourceDetails.mcp.list.none', 'resourceDetails.mcp.list.noneLead', 'resourceDetails.mcp.list.noMatch',
    'resourceDetails.mount.list.lead', 'resourceDetails.mount.list.search', 'resourceDetails.mount.list.none', 'resourceDetails.mount.list.noneLead', 'resourceDetails.mount.list.noMatch',
  ] },
];

describe('ordinary catalogue ownership', () => {
  it('rejects absent-part promises in both languages while retaining real data/state words', () => {
    for (const phrase of ['not built', 'not yet available', 'not yet supported', 'not yet implemented', 'this build', 'from this server', 'not served', 'available yet', 'supported yet', 'possible yet', 'part of this build', 'fill this section', 'does not take', '此服务器暂时不提供搜索', '这个功能尚未实现']) expect(UNFINISHED_PRODUCT.test(phrase), phrase).toBe(true);
    for (const phrase of ['No messages yet', 'The machine has not reported yet', '还没有消息', '此程序未提供终端功能', 'This program does not offer a terminal']) expect(UNFINISHED_PRODUCT.test(phrase), phrase).toBe(false);
  });

  it('finds unreferenced keys without treating a comment, prose or an orphan alias as a reader', () => {
    const catalogue = { common: { live: 'Open', dead: 'Gone', prose: 'Gone', alias: '$t(common.live)', orphan: '$t(common.dead)' },
      count_one: 'One', count_other: 'Many', dynamic: { first: { title: 'First', unused: 'Gone' } }, late: { name: 'Late' } };
    const source = new Map([['screen.tsx', ["t('common.alias');", "const caption='count'; t(caption,{count:n});", 't(`dynamic.${kind}.title`);',
      "// t('common.dead');", 'const prose="t(\'common.prose\')";'].join('\n')]]);
    expect(unusedMessages(catalogue, source, [])).toEqual(['common.dead', 'common.orphan', 'common.prose', 'dynamic.first.unused', 'late.name']);
    const declared = [{ source: 'screen.tsx', keys: ['late.name'], reason: 'An externally supplied name key on this screen' }];
    expect(unusedMessages(catalogue, source, declared)).not.toContain('late.name');
    expect(unusedMessages(catalogue, new Map(), declared)).toContain('late.name');
  });

  it('follows a translated prefix without treating a similarly named cache template as a reader', () => {
    const catalogue = { scope: { org: { title: 'Org', obsolete: 'Gone' }, cacheOnly: 'Gone' } };
    const source = new Map([['screen.tsx', "const base='scope.org'; t(`${base}.title`); const cache=`scope.${id}`;"]]);
    expect(unusedMessages(catalogue, source, [])).toEqual(['scope.cacheOnly', 'scope.org.obsolete']);
  });

  it('keeps a declared finite translated vocabulary from retaining the rest of its namespace', () => {
    const catalogue = { count: { added_one: 'One', added_other: 'Many', removed: 'Removed', obsolete: 'Gone' } };
    const source = new Map([['screen.tsx', "function say(key: 'added' | 'removed') { return t(`count.${key}`); }"]]);
    expect(unusedMessages(catalogue, source, [])).toEqual(['count.obsolete']);
  });

  it('follows aliases discovered during traversal through chains, cycles and plural families', () => {
    const catalogue = { entry: '$t(labels.first)', labels: { first: '$t(labels.second)', second: '$t(items)',
      cycle: '$t(labels.cycle)', dead: 'Unused' }, items_one: 'One', items_other: 'Many' };
    const source = new Map([['screen.tsx', "t('entry');"]]);
    expect(unusedMessages(catalogue, source, [])).toEqual(['labels.cycle', 'labels.dead']);
    source.set('screen.tsx', "t('entry'); t('labels.cycle');");
    expect(unusedMessages(catalogue, source, [])).toEqual(['labels.dead']);
  });

  it('matches large catalogues by dynamic readers without testing every literal against every key', () => {
    for (const [total, literal] of [[2_000, 200], [100_000, 1_000]] as const) {
      const catalogue = { scale: Object.fromEntries(Array.from({ length: total }, (_, index) => [
        index < literal ? `live${index}` : `dead${index}`, 'A translated value',
      ])) };
      const text = Array.from({ length: literal }, (_, index) => `t('scale.live${index}');`).join('\n')
        + '\nt(`scale.dynamic.${kind}`);';
      const sources = new Map([['screen.tsx', text]]);
      const native = Object.getOwnPropertyDescriptor(RegExp.prototype, 'test');
      const ordinary: unknown = native?.value;
      if (native === undefined || typeof ordinary !== 'function') throw new Error('The native regex test method is available.');
      let matches = 0;
      RegExp.prototype.test = function (this: RegExp, value: string) {
        if (this.source.startsWith('^scale')) matches += 1;
        return Reflect.apply(ordinary, this, [value]) === true;
      };
      try {
        const readers = catalogueReaders(sources, []);
        expect(unusedMessages(catalogue, sources, [], readers)).toHaveLength(total - literal);
      } finally { Object.defineProperty(RegExp.prototype, 'test', native); }
      // One dynamic reader tries the actual key and its plural base; fixed readers are point lookups.
      expect(matches).toBeLessThanOrEqual(total * 2);
    }
  }, SOURCE_AUDIT_MS);

  it('holds no unfinished-product message or unreferenced key in either ordinary catalogue', () => {
    const sources = productSources(resolve(import.meta.dirname, '../../src'));
    const readers = catalogueReaders(sources, RUNTIME_MESSAGES);
    const failures: string[] = [];
    for (const [locale, catalogue] of [['en', english], ['zh-CN', chinese]] as const) {
      for (const [key, text] of messages(catalogue)) if (UNFINISHED_PRODUCT.test(text)) failures.push(`${locale}:${key} promises an absent product part: ${text}`);
      failures.push(...unusedMessages(catalogue, sources, RUNTIME_MESSAGES, readers).map((key) => `${locale}:${key} has no product reader`));
    }
    expect(failures, failures.join('\n')).toEqual([]);
  }, SOURCE_AUDIT_MS);
});
