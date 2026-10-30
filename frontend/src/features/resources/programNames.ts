/** The name supplied for a program, in the reader's language. */
import type { HarnessKindInfo, HarnessKindName } from '@/generated/api-types';
import { reportedHarnessName } from '@/features/resources/agents/descriptor';

/** Older saved responses can precede localized names; current responses carry the same fields plus names. */
export type DeclaredProgramName = Omit<HarnessKindName, 'names'> & { names?: Record<string, string> };

/** Validate the named kind carried by an otherwise unknown response. */
export function isProgramName(value: unknown): value is DeclaredProgramName {
  return typeof value === 'object' && value !== null && 'kind' in value && typeof value.kind === 'string'
    && 'name' in value && typeof value.name === 'string'
    && (!('names' in value) || value.names === undefined
      || (typeof value.names === 'object' && value.names !== null && !Array.isArray(value.names)));
}

/** The declared English name is the fallback when a translation is absent. */
export function programName(program: DeclaredProgramName | null | undefined, locale: string): string | null {
  if (program === null || program === undefined) return null;
  const translated = program.names?.[locale];
  if (typeof translated === 'string' && translated.trim() !== '') return translated;
  const fallback = program.name;
  return fallback.trim() === '' ? null : fallback;
}

/** Read the named harness carried by a response or a referenced profile. */
export function programNameIn(value: unknown, locale: string): string | null {
  return typeof value === 'object' && value !== null && 'harness' in value && isProgramName(value.harness)
    ? programName(value.harness, locale) : null;
}

/** A registry row's named harness wins over older descriptor and title fields. */
export function programInfoName(info: HarnessKindInfo | null | undefined, locale: string): string | null {
  if (info === null || info === undefined) return null;
  const named = 'harness' in info && isProgramName(info.harness) ? info.harness : null;
  const name = programName(named, locale) ?? reportedHarnessName(info.descriptor, locale) ?? info.title;
  return name.trim() === '' ? null : name;
}

// Several visible panels may observe the same response; its index is shared and dies with that response.
const registryIndexes = new WeakMap<readonly HarnessKindInfo[], { locale: string; names: ReadonlyMap<string, string | null> }>();

/** A registry snapshot is indexed once in the current language, rather than once per cap or status label. */
export function programNamesIndex(programs: readonly HarnessKindInfo[], locale: string): ReadonlyMap<string, string | null> {
  const previous = registryIndexes.get(programs);
  if (previous?.locale === locale) return previous.names;
  const names = new Map<string, string | null>();
  for (const program of programs) names.set(program.kind, programInfoName(program, locale));
  registryIndexes.set(programs, { locale, names });
  return names;
}
