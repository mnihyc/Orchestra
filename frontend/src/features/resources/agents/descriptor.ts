/** What an agent profile's harness declares about itself, read with care: its adapter wrote it, and a profile whose harness has not yet reported has nothing.
 */

import type { TFunction } from 'i18next';

/** A build's support status in the reader's language, including an unreported extension value. */
export function harnessSupportWords(t: TFunction, status: string): string {
  switch (status) {
    case 'supported': return t('agents.overview.supported');
    case 'untested': return t('agents.overview.untested');
    case 'unsupported': return t('agents.overview.unsupported');
    default: return t('agents.overview.supportUnknown');
  }
}

/** A version identifier stays literal; an absent or unknown build is explained as unreported. */
export function harnessVersionWords(t: TFunction, version: string | null): string {
  return version === null || version.trim() === '' || version.toLowerCase() === 'unknown' ? t('agents.overview.versionUnknown') : version;
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const text = (value: unknown): string | null => (typeof value === 'string' ? value : null);

/** A reported program name is a declaration, never a guess from its kind. */
export function reportedHarnessName(value: unknown, locale = 'en'): string | null {
  if (!isRecord(value) || !isRecord(value.harness)) return null;
  const translations = isRecord(value.text) && isRecord(value.text[locale]) ? value.text[locale] : null;
  const translated = translations === null ? null : text(translations['harness.name']);
  if (translated !== null && translated.trim() !== '') return translated;
  const name = text(value.harness.name);
  return name === null || name.trim() === '' ? null : name;
}

export interface HarnessSupport {
  name: string;
  /** The adapter's word for how well the build is supported: `supported`, or another it names. */
  status: string;
  version: string | null;
  range: string | null;
}

/** A parameter the harness publishes, which a person may set for a place or a thread. */
export interface ParameterDef {
  key: string;
  type: string;
  /** When a change takes effect: `next_turn`, or another the adapter names. */
  apply: string;
  group: string;
  title: string;
  scopes: string[];
  description: string;
  /** Its starting value, written as a short phrase. */
  default: string;
  /** What it may be, written as a short phrase. */
  options: string;
}

export interface ArrangementOffer {
  type: string;
  executor: string | null;
  toolSets: string[];
}

export interface DescriptorView {
  /** Whether the harness has said anything at all. */
  declared: boolean;
  harness: HarnessSupport | null;
  parameters: ParameterDef[];
  /** What the harness can do, as the adapter lists it: a name, and what is said of it. */
  capabilities: { key: string; value: string }[];
  arrangements: ArrangementOffer[];
  sandboxModes: string[];
  approvalModes: string[];
  adapterVersion: number | null;
}

const words = (value: unknown): string[] => (Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []);

/** A declared value as a short phrase: words as they are, a choice by its labels, a reference to the model's own as "from the model". */
export function phrase(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) {
    return value
      .map((entry) => (isRecord(entry) ? (text(entry.label) ?? text(entry.value) ?? JSON.stringify(entry)) : phrase(entry)))
      .join(', ');
  }
  if (isRecord(value)) {
    const from = text(value.from_model);
    if (from !== null) return `← ${from}`;
    return Object.entries(value)
      .map(([key, inner]) => `${key}: ${phrase(inner)}`)
      .join(', ');
  }
  return '';
}

/** A capability's key as people read it: `fork_native` is "Fork native". */
export function humanize(key: string): string {
  const spaced = key.replaceAll('_', ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

export function readDescriptor(value: unknown): DescriptorView {
  if (!isRecord(value) || Object.keys(value).length === 0) {
    return { declared: false, harness: null, parameters: [], capabilities: [], arrangements: [], sandboxModes: [], approvalModes: [], adapterVersion: null };
  }
  const harness = isRecord(value.harness) ? value.harness : null;
  const parameters = Array.isArray(value.parameters) ? value.parameters : [];
  const arrangements = Array.isArray(value.arrangements) ? value.arrangements : [];
  const capabilities = isRecord(value.capabilities) ? Object.entries(value.capabilities) : [];
  return {
    declared: true,
    harness:
      harness === null
        ? null
        : { name: text(harness.name) ?? '', status: text(harness.status) ?? '', version: text(harness.version), range: text(harness.supported_range) },
    parameters: parameters.flatMap((entry): ParameterDef[] => {
      if (!isRecord(entry)) return [];
      const key = text(entry.key);
      if (key === null) return [];
      return [
        {
          key,
          type: text(entry.type) ?? '',
          apply: text(entry.apply) ?? '',
          group: text(entry.group) ?? '',
          title: text(entry.title) ?? key,
          scopes: words(entry.scopes),
          description: text(entry.description) ?? '',
          default: phrase(entry.default),
          options: phrase(entry.options),
        },
      ];
    }),
    capabilities: capabilities.map(([key, inner]) => ({ key, value: phrase(inner) })),
    arrangements: arrangements.flatMap((entry): ArrangementOffer[] => {
      if (!isRecord(entry)) return [];
      const type = text(entry.type);
      return type === null ? [] : [{ type, executor: text(entry.executor), toolSets: words(entry.tool_sets) }];
    }),
    sandboxModes: words(value.sandbox_modes),
    approvalModes: words(value.approval_modes),
    adapterVersion: typeof value.adapter_version === 'number' ? value.adapter_version : null,
  };
}

/** The version a profile pins its harness to, or nothing where it takes the default build. */
export function versionPin(profile: unknown): string | null {
  const pin = isRecord(profile) ? text(profile.harness_version) : null;
  return pin === null || pin.trim() === '' ? null : pin;
}
