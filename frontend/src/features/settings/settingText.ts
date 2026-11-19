/** Registry wording and numeric display come from catalogue and declared metadata, while technical keys and user values stay intact. */
import type { i18n, TFunction } from 'i18next';
import { isUnit, shapeOf, type Shape, type Unit } from '@/features/settings/settingsModel';
import { formatBytes, formatNumber } from '@/lib/format';
import type { SettingDef as SettingWithText } from '@/generated/api-types';

const STRUCTURED_FIELDS = { cpu_millis: 'access.options.cpu_millis', memory_mb: 'access.options.memory_mb', memory_swap_mb:'access.options.memory_swap_mb', blkio_weight:'access.options.blkio_weight', pids: 'access.options.pids', disk_gb: 'access.options.disk_gb' } as const;
function knownField(name: string): name is keyof typeof STRUCTURED_FIELDS { return Object.hasOwn(STRUCTURED_FIELDS, name); }
const RELATED_FIELDS = { meaning: 'related.fields.meaning', place: 'related.fields.place', references: 'related.fields.references', folder: 'related.fields.folder', topic: 'related.fields.topic', project: 'related.fields.project', org: 'related.fields.org' } as const;
function knownRelatedField(name: string): name is keyof typeof RELATED_FIELDS { return Object.hasOwn(RELATED_FIELDS, name); }

const SEARCH_FIELDS = {
  provider_id: 'admin.semantic.provider', model_name: 'admin.semantic.fields.model', dimensions: 'admin.semantic.fields.dimensions',
  query_prefix: 'admin.semantic.fields.queryPrefix', document_prefix: 'admin.semantic.fields.documentPrefix',
  chunk_tokens: 'admin.semantic.fields.chunkTokens', batch_tokens: 'admin.semantic.fields.batchTokens',
  candidates: 'admin.semantic.fields.candidates', oversample: 'admin.semantic.fields.oversample',
  fusion_k: 'admin.semantic.fields.fusionK', same_model_cosine_milli: 'admin.semantic.fields.similarity',
  interval_secs: 'admin.semantic.fields.interval', query_wait_secs: 'admin.semantic.fields.queryWait',
  exact_filter_candidates: 'admin.semantic.fields.exactFilter', source_batch: 'admin.semantic.fields.sourceBatch',
  max_scan_tuples: 'admin.semantic.fields.scanVisits', scan_mem_multiplier: 'admin.semantic.fields.scanMemory',
  scope_candidates: 'searchTuning.fields.scopeRows', tiers: 'searchTuning.fields.fieldLimits',
  request_windows: 'searchTuning.fields.requestWindows',
  title: 'searchTuning.fields.title', description: 'searchTuning.fields.description',
  heading: 'searchTuning.fields.heading', body: 'searchTuning.fields.body',
  ef_search: 'searchTuning.fields.graphBreadth', iterative_scan: 'searchTuning.fields.graphScan',
} as const;
function knownSearchField(name: string): name is keyof typeof SEARCH_FIELDS { return Object.hasOwn(SEARCH_FIELDS, name); }

/** Search controls, summaries and full entries name their structured fields in the same words. */
export function searchFieldName(name: string, t: TFunction): string | undefined {
  return knownSearchField(name) ? t(SEARCH_FIELDS[name]) : undefined;
}

/** Built-in resource members have translated names; variables keep the names their author entered. */
export function settingFieldName(name: string, key: string | undefined, t: TFunction): string {
  if ((key === 'related.weights' || key === 'related.place_scores') && knownRelatedField(name)) return t(RELATED_FIELDS[name]);
  if (key === 'search.word_tuning' || key === 'search.semantic.tuning' || key === 'search.semantic.embeddings') return searchFieldName(name, t) ?? name.replaceAll('_', ' ').replaceAll('.', ' ');
  return key === 'env.variables' || key === 'runtime.variables' ? name : knownField(name) ? t(STRUCTURED_FIELDS[name]) : name.replaceAll('_', ' ').replaceAll('.', ' ');
}

/** Both shipped catalogues contribute search words, while the setting keeps its technical identity. */
function keywordsOf(def: SettingWithText, instance: i18n): string[] {
  const fallback = `day100.builtin.${def.key}`;
  const words = ['en', 'zh-CN'].flatMap((locale): string[] => {
    const title: unknown = instance.getResource(locale, 'translation', `settingRegistry.${def.key}.title`);
    const declared: unknown = def.text_key === null ? undefined : instance.getResource(locale, 'translation', `${def.text_key}.keywords`);
    const found: unknown = declared ?? instance.getResource(locale, 'translation', `${fallback}.keywords`);
    const keywords = typeof found === 'string' ? [found] : Array.isArray(found) ? found.filter((word: unknown): word is string => typeof word === 'string') : typeof found === 'object' && found !== null ? Object.values<unknown>(found as Record<string, unknown>).filter((word): word is string => typeof word === 'string') : [];
    return [...(typeof title === 'string' ? [title] : []), ...keywords];
  });
  return [...new Set([...def.keywords, ...words])];
}

/** A built-in key has a stable local fallback while older registries have no text_key. Extension words remain their own. */
export function localizedSetting(def: SettingWithText, instance: i18n, language = instance.resolvedLanguage ?? 'en'): SettingWithText {
  if (def.key === 'providers.allowed') {
    const title: unknown = instance.getResource(language, 'translation', 'providerPolicy.settingTitle');
    const description: unknown = instance.getResource(language, 'translation', 'providerPolicy.settingHelp');
    if (typeof title === 'string' && typeof description === 'string') return { ...def, title, description, keywords: keywordsOf(def, instance) };
  }
  const fallback = `day100.builtin.${def.key}`;
  // exists() follows fallbackLng: it can mistake an English entry for a Chinese translation.
  const candidates = [`settingRegistry.${def.key}`, ...(def.text_key === null ? [] : [def.text_key]), fallback];
  const base = candidates.find((path) => ['title', 'description'].every((field) => {
    const value: unknown = instance.getResource(language, 'translation', `${path}.${field}`);
    return typeof value === 'string' && value.trim() !== '';
  }));
  if (base === undefined) return def;
  const title: unknown = instance.getResource(language, 'translation', `${base}.title`);
  const description: unknown = instance.getResource(language, 'translation', `${base}.description`);
  return { ...def, title: typeof title === 'string' ? title : def.title, description: typeof description === 'string' ? description : def.description, keywords: keywordsOf(def,instance) };
}

/** Display metadata can refine a schema's declared unit; a raw key never acquires a guessed suffix-based unit. */
export function settingShape(def: SettingWithText): Shape {
  const shape = shapeOf(def.schema);
  return shape.type === 'int' && def.display !== undefined && def.display !== null ? { ...shape, unit: def.display.unit === 'raw' ? null : def.display.unit } : shape;
}

interface NumericDisplay { unit: Unit | 'raw'; zero: NonNullable<SettingWithText['display']>['zero'] }

/** Explicit display metadata wins; a nullable number otherwise retains its schema's declared unit. */
export function numericSettingDisplay(def: Partial<Pick<SettingWithText, 'display' | 'schema'>>): NumericDisplay | null {
  if (def.display !== undefined && def.display !== null) return def.display;
  let shape = shapeOf(def.schema);
  while (shape.type === 'nullable') shape = shape.of;
  return shape.type === 'int' && isUnit(shape.unit) ? {unit: shape.unit, zero: null} : null;
}

/** A zero's declared meaning supplies its word; a zero-day removal remains zero days. */
export function numericSettingText(value: number, display: NumericDisplay, locale: string, t: TFunction): string {
  if (value === 0 && display.zero === 'never') return t('day100.settings.never');
  if (value === 0 && display.zero === 'off') return t('common.off');
  if (value === 0 && display.zero === 'no_limit') return t('goals.editor.noLimit');
  if (display.unit === 'bytes') return formatBytes(value, locale);
  const number = formatNumber(value, locale);
  return display.unit === 'raw' ? number : `${number} ${t(`settings.editor.units.${display.unit}`)}`;
}
