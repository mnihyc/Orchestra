/** Provider help ids are shared by controls, setting entries, search and the content gate. */
import type { TranslationKey } from '@/i18n';

export type ProviderGuideId = 'provider-details' | 'commands-after-start' | 'provider-readme'
  | 'provider-facts' | 'provider-models' | 'provider-resets' | 'provider-credential';
export const PROVIDER_GUIDES: readonly { id: ProviderGuideId; title: TranslationKey }[] = [
  { id: 'provider-details', title: 'helpProvider.details' },
  { id: 'commands-after-start', title: 'helpProvider.commands' },
  { id: 'provider-readme', title: 'helpProvider.overview' },
  { id: 'provider-facts', title: 'helpProvider.facts' },
  { id: 'provider-models', title: 'helpProvider.models' },
  { id: 'provider-resets', title: 'helpProvider.resets' },
  { id: 'provider-credential', title: 'helpProvider.credential' },
];

/** Worked programs are indexed in Build your own; their source remains an example to inspect. */
export const PROVIDER_EXAMPLES: readonly { id: string; title: TranslationKey; words: readonly string[] }[] = [
  { id: 'provider-codex-plan-example', title: 'helpProvider.codexExample', words: ['Codex', 'subscription example', 'Python', '订阅示例', '额度'] },
  { id: 'provider-claude-plan-example', title: 'helpProvider.claudeExample', words: ['Claude Code', 'subscription example', 'Python', '订阅示例', '额度'] },
  { id: 'provider-openrouter-key-example', title: 'helpProvider.openRouterExample', words: ['OpenRouter', 'API key', 'Python', '密钥示例', '预算', '用量'] },
];

const UNIT_SETTINGS = ['env.after_up', 'runtime.after_up', 'runtime.storage', 'runtime.needs',
  'env.variables', 'env.secret_names', 'runtime.variables', 'runtime.secret_names'];

export function providerGuidesForSetting(key: string): ProviderGuideId[] {
  if (UNIT_SETTINGS.includes(key)) return ['commands-after-start'];
  if (key === 'providers.allow_http_upstreams') return ['provider-details'];
  return [];
}
