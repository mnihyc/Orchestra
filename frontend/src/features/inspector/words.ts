/** The words for a value the API names, and for a stand-in when a newer server names one this page has none for.
 *
 * A kind, a status or a reason arrives as the API's own name. A page that asked the catalogue for a name it
 * does not hold would stop (or show the key), so the views ask here and fall back to the words of a value
 * they do know.
 */
import type { TFunction } from 'i18next';
import type { InstructionEvidence, InstructionSourceKind, ModelInputPurpose, ModelInputState } from '@/generated/api-types';

import { i18n, type TranslationKey } from '@/i18n';

/** The text under `family` for `value`, or the text for `standIn` where the catalogue has none for `value`. */
function wordFor(t: TFunction, family: string, value: string | null | undefined, standIn: string): string {
  const key = `${family}.${value ?? ''}`;
  // The key is built from a name the server gave, so it is not one the catalogue's types know.
  const named = value !== null && value !== undefined && i18n.exists(key) ? key : `${family}.${standIn}`;
  return t(named as TranslationKey);
}

/** Why a model cannot be chosen. */
export const refusalWord = (t: TFunction, reason: string | null | undefined): string => wordFor(t, 'start.reason', reason, 'disabled');

/** When a parameter's new value takes effect. */
export const applyWord = (t: TFunction, mode: string): string => wordFor(t, 'composer.model.applies', mode, 'next_turn');

/** What kind of line an entry of the outline is. */
export const outlineKindWord = (t: TFunction, kind: string): string => wordFor(t, 'inspector.outline.kind', kind, 'marker');

/** How an environment stands. */
export const environmentStatusWord = (t: TFunction, status: string): string => wordFor(t, 'environments.status', status, 'none');

// Explicit keys keep native enum words in the catalogue and make additions type checked.
const BLOCK_WORDS = {
  preamble: 'threadViews.given.blocks.preamble',
  tools_guide: 'threadViews.given.blocks.tools_guide',
  standing: 'threadViews.given.blocks.standing',
  standing_heading: 'threadViews.given.blocks.standing_heading',
  where_you_are: 'threadViews.given.blocks.where_you_are',
  input: 'threadViews.given.blocks.input',
  side_reading: 'threadViews.given.blocks.side_reading',
  turn_stopped: 'threadViews.given.blocks.turn_stopped',
  turn_usage_limit: 'threadViews.given.blocks.turn_usage_limit',
  turn_environment_changed: 'threadViews.given.blocks.turn_environment_changed',
  turn_topic_setting: 'threadViews.given.blocks.turn_topic_setting',
  attribution_started: 'threadViews.given.blocks.attribution_started',
  pending_notices: 'threadViews.given.blocks.pending_notices',
  frames: 'threadViews.given.blocks.frames',
  other: 'threadViews.given.blocks.other',
} satisfies Record<string, TranslationKey>;

export const givenBlockWord = (t: TFunction, value: string): string => t(((BLOCK_WORDS as Readonly<Record<string, TranslationKey | undefined>>)[value]) ?? BLOCK_WORDS.other);

const PURPOSE_WORDS = {
  session: 'threadViews.given.purpose.session',
  input: 'threadViews.given.purpose.input',
  tool: 'threadViews.given.purpose.tool',
  summary: 'threadViews.given.purpose.summary',
} satisfies Record<ModelInputPurpose, TranslationKey>;

export const givenPurposeWord = (t: TFunction, value: string): string => t(((PURPOSE_WORDS as Readonly<Record<string, TranslationKey | undefined>>)[value]) ?? PURPOSE_WORDS.input);

const STATE_WORDS = {
  prepared: 'threadViews.given.state.prepared',
  sent: 'threadViews.given.state.sent',
  failed: 'threadViews.given.state.failed',
  uncertain: 'threadViews.given.state.uncertain',
} satisfies Record<ModelInputState, TranslationKey>;

export const givenStateWord = (t: TFunction, value: string): string => t(((STATE_WORDS as Readonly<Record<string, TranslationKey | undefined>>)[value]) ?? STATE_WORDS.uncertain);

const SOURCE_WORDS = {
  preamble: 'threadViews.given.sources.kind.preamble',
  standing: 'threadViews.given.sources.kind.standing',
  guide: 'threadViews.given.sources.kind.guide',
  file: 'threadViews.given.sources.kind.file',
  memory: 'threadViews.given.sources.kind.memory',
} satisfies Record<InstructionSourceKind, TranslationKey>;

export const sourceKindWord = (t: TFunction, value: string): string => t(((SOURCE_WORDS as Readonly<Record<string, TranslationKey | undefined>>)[value]) ?? SOURCE_WORDS.file);

const EVIDENCE_WORDS = {
  orch: 'threadViews.given.sources.evidence.orch',
  reported: 'threadViews.given.sources.evidence.reported',
  documented_rule: 'threadViews.given.sources.evidence.documented_rule',
} satisfies Record<InstructionEvidence, TranslationKey>;

export const sourceEvidenceWord = (t: TFunction, value: string): string => t(((EVIDENCE_WORDS as Readonly<Record<string, TranslationKey | undefined>>)[value]) ?? EVIDENCE_WORDS.reported);
