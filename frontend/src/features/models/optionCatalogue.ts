/** Documented wire fields are editing suggestions; native model/endpoint constraints still decide each request. */
export type OptionKind = 'boolean' | 'string' | 'strings' | 'object' | 'objects';
export type OptionId = 'safetyIdentifier' | 'user' | 'metadata' | 'cacheKey' | 'cacheRetention' | 'cacheMode' | 'cacheLifetime' | 'routingOrder' | 'routingOnly' | 'routingIgnore' | 'routingFallbacks' | 'routingRequired' | 'routingData' | 'routingSort' | 'models' | 'plugins' | 'transforms' | 'tier' | 'verbosity' | 'store' | 'googleSafety' | 'googleCache' | 'speed' | 'cacheType' | 'metadataUser';
export interface DocumentedOption {
  id: OptionId;
  path: readonly string[];
  kind: OptionKind;
  values?: readonly string[];
  identifier?: boolean;
  extension?: boolean;
}
const field = (id: OptionId, path: string, kind: OptionKind, extra: Omit<Partial<DocumentedOption>, 'id' | 'path' | 'kind'> = {}): DocumentedOption => ({ id, path: path.split('.'), kind, ...extra });
const ids = [field('safetyIdentifier', 'safety_identifier', 'string', { identifier: true }), field('user', 'user', 'string', { identifier: true }), field('metadata', 'metadata', 'object')];
const cache = [field('cacheKey', 'prompt_cache_key', 'string'), field('cacheRetention', 'prompt_cache_retention', 'string', { values: ['in_memory', '24h'] }), field('cacheMode', 'prompt_cache_options.mode', 'string', { values: ['implicit', 'explicit'] })];
const routing = [
  field('routingOrder', 'provider.order', 'strings', { extension: true }),
  field('routingOnly', 'provider.only', 'strings', { extension: true }),
  field('routingIgnore', 'provider.ignore', 'strings', { extension: true }),
  field('routingFallbacks', 'provider.allow_fallbacks', 'boolean', { extension: true }),
  field('routingRequired', 'provider.require_parameters', 'boolean', { extension: true }),
  field('routingData', 'provider.data_collection', 'string', { extension: true, values: ['allow', 'deny'] }),
  field('routingSort', 'provider.sort', 'string', { extension: true, values: ['price', 'throughput', 'latency'] }),
  field('models', 'models', 'strings', { extension: true }), field('plugins', 'plugins', 'objects', { extension: true }), field('transforms', 'transforms', 'strings', { extension: true }),
];
const openaiTiers = ['auto', 'default', 'flex', 'scale', 'priority', 'fast'];
const catalogue: Readonly<Record<string, readonly DocumentedOption[]>> = {
  'openai-responses': [
    field('tier', 'service_tier', 'string', { values: [...openaiTiers, 'ultrafast'] }),
    field('verbosity', 'text.verbosity', 'string', { values: ['low', 'medium', 'high'] }),
    ...cache, field('cacheLifetime', 'prompt_cache_options.ttl', 'string', { values: ['30m', '12h'] }), ...ids,
    field('store', 'store', 'boolean'), ...routing,
  ],
  'openai-chat': [
    field('tier', 'service_tier', 'string', { values: openaiTiers }),
    field('verbosity', 'verbosity', 'string', { values: ['low', 'medium', 'high'] }),
    ...cache, field('cacheLifetime', 'prompt_cache_options.ttl', 'string', { values: ['30m'] }), ...ids,
    field('store', 'store', 'boolean'), ...routing,
    field('googleSafety', 'extra_body.google.safety_settings', 'objects', { extension: true }),
    field('googleCache', 'extra_body.google.cached_content', 'string', { extension: true }),
  ],
  'anthropic-messages': [
    field('tier', 'service_tier', 'string', { values: ['auto', 'standard_only'] }),
    field('speed', 'speed', 'string', { values: ['standard', 'fast'] }),
    field('cacheType', 'cache_control.type', 'string', { values: ['ephemeral'] }),
    field('cacheLifetime', 'cache_control.ttl', 'string', { values: ['5m', '1h'] }),
    field('metadata', 'metadata', 'object'), field('metadataUser', 'metadata.user_id', 'string', { identifier: true }), ...routing,
  ],
};
export function documentedOptions(protocol: string): readonly DocumentedOption[] { return catalogue[protocol] ?? []; }

/** Fixed headers use the endpoint's real header map, not model body schemas or secret values. */
export function documentedHeaders(protocol: string): readonly string[] {
  return protocol === 'anthropic-messages' ? ['anthropic-beta', 'anthropic-workspace-id', 'anthropic-version']
    : protocol === 'openai-responses' || protocol === 'openai-chat' ? ['OpenAI-Project', 'OpenAI-Organization'] : [];
}

/** Suggestions never write a default, constant or paid start by themselves. */
export function suggestedRule(option: DocumentedOption): Record<string, unknown> {
  if (option.kind === 'strings') return { type: 'array', items: { type: 'string' } };
  if (option.kind === 'objects') return { type: 'array', items: { type: 'object' } };
  return { type: option.kind };
}
