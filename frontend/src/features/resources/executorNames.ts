/** The executor and tool-set words supplied with an arrangement or readiness report. */
import type { TFunction } from 'i18next';
import type { Isolation } from '@/generated/api-types';

/** Select declared words in the reader's language, with their English fallback. */
export function localizedText(value: unknown, locale: string): string | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const words = value as Record<string, unknown>;
  const translated = words[locale];
  if (typeof translated === 'string' && translated.trim() !== '') return translated;
  return typeof words.en === 'string' && words.en.trim() !== '' ? words.en : null;
}

/** A selected tool set names itself; otherwise the executor names itself. */
export function executorTitle(reference: unknown, locale: string): string | null {
  if (typeof reference !== 'object' || reference === null || Array.isArray(reference)) return null;
  const data = reference as Record<string, unknown>;
  const toolSet = typeof data.tool_set === 'object' && data.tool_set !== null && !Array.isArray(data.tool_set)
    ? data.tool_set as Record<string, unknown> : null;
  return localizedText(toolSet?.title, locale) ?? localizedText(data.title, locale);
}

/** Name an executor or one of its tool sets declared in the program's registry entry. */
export function declaredExecutorTitle(program: unknown, kind: string, toolSet: string | null, locale: string): string | null {
  const executor = declaredExecutorNames(program, locale).get(kind);
  return toolSet === null ? executor?.title ?? null : executor?.toolSets.get(toolSet) ?? null;
}

/** Index declarations once so a page's labels do not repeatedly scan every executor and tool set. */
export function declaredExecutorNames(program: unknown, locale: string) {
  const names = new Map<string, { title: string | null; toolSets: Map<string, string | null> }>();
  if (typeof program !== 'object' || program === null || !('executors' in program) || !Array.isArray(program.executors)) return names;
  for (const executor of program.executors as unknown[]) {
    if (typeof executor !== 'object' || executor === null || !('kind' in executor) || typeof executor.kind !== 'string') continue;
    const toolSets = new Map<string, string | null>();
    if ('tool_sets' in executor && Array.isArray(executor.tool_sets)) for (const toolSet of executor.tool_sets as unknown[]) {
      if (typeof toolSet === 'object' && toolSet !== null && 'name' in toolSet && typeof toolSet.name === 'string') {
        toolSets.set(toolSet.name, 'title' in toolSet ? localizedText(toolSet.title, locale) : null);
      }
    }
    names.set(executor.kind, { title: 'title' in executor ? localizedText(executor.title, locale) : null, toolSets });
  }
  return names;
}

/** Describe the actual executor without inferring ownership from its identifier. */
export function isolationToolWords(isolation: Isolation, t: TFunction, locale: string, agent?: string): string | null {
  if (isolation.tools !== 'executor') return null;
  const reference = 'executor_ref' in isolation ? isolation.executor_ref : null;
  const name = executorTitle(reference, locale);
  if (name === null) return t('thread.isolation.tools.unreported');
  return agent === undefined ? t('thread.isolation.tools.named', { name }) : t('thread.isolation.tools.namedFor', { name, agent });
}
