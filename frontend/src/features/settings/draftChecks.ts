/** What makes a draft of a structured value fit to save, and what it is when it says "nothing here", for the two settings that have an editor of their own.
 *
 * The editors draw these problems beside the fields they belong to; the row only asks whether there are any, to keep its Save off until there are none.
 */

export const SUMMARIZER_SETTING = 'auto.summarizer';
export const SUMMARY_TARGETS_SETTING = 'summary.targets';

export const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The one kind of summarizer that ships: it asks a model through a provider. */
export const MODEL_SUMMARIZER = 'model';

/** A summarizer chosen as "none here": the place says nothing, and what is above applies. */
export const NO_SUMMARIZER: Readonly<Record<string, unknown>> = { kind: '' };

/** The members of a model summarizer that are numbers of at least one, whose absence means the built-in count. */
export const SUMMARIZER_COUNTS = ['max_output_tokens', 'context_tokens'] as const;

/** Whether a summarizer value says "none here". */
export function noSummarizer(value: unknown): boolean {
  return value === null || value === undefined || isRecord(value) && (value.kind === '' || value.kind === undefined) && Object.keys(value).every((name) => name === 'kind');
}

const positive = (value: unknown): boolean => typeof value === 'number' && Number.isInteger(value) && value >= 1;

/** The members of a model summarizer that cannot be saved as they stand: a provider and a model are needed, and counts are whole numbers from one. */
export function summarizerProblems(value: unknown): string[] {
  if (!isRecord(value) || value.kind !== MODEL_SUMMARIZER) return [];
  const problems: string[] = [];
  if (typeof value.provider_id !== 'string' || value.provider_id === '') problems.push('provider_id');
  if (typeof value.model_name !== 'string' || value.model_name.trim() === '') problems.push('model_name');
  for (const member of SUMMARIZER_COUNTS) if (member in value && !positive(value[member])) problems.push(member);
  return problems;
}

/** The targets whose automatic refresh has a place of its own in the editor, in the order they are drawn. */
export const SUMMARY_TARGETS = ['thread', 'topic', 'document', 'chat', 'chat_title'] as const;
export type SummaryTarget = (typeof SUMMARY_TARGETS)[number];

/** The members of a target's entry that are numbers, and the least each may be: a gap may be none, a size is at least one. */
export const TARGET_NUMBERS = [['min_interval_secs', 0], ['context_tokens', 1], ['output_chars', 1]] as const;
export type TargetNumber = (typeof TARGET_NUMBERS)[number][0];

/** The entries of a draft of the summary targets whose numbers are not whole numbers from their least. */
export function targetProblems(value: unknown): { target: string; member: string }[] {
  if (!isRecord(value)) return [];
  const problems: { target: string; member: string }[] = [];
  for (const target of SUMMARY_TARGETS) {
    const entry = value[target];
    if (!isRecord(entry)) continue;
    for (const [member, least] of TARGET_NUMBERS) {
      const given = entry[member];
      if (member in entry && !(typeof given === 'number' && Number.isInteger(given) && given >= least)) problems.push({ target, member });
    }
  }
  return problems;
}

/** Whether a draft of this setting has nothing in it that cannot be saved. */
export function draftComplete(key: string, value: unknown): boolean {
  if (key === SUMMARIZER_SETTING) return summarizerProblems(value).length === 0;
  if (key === SUMMARY_TARGETS_SETTING) return targetProblems(value).length === 0;
  return true;
}

/** Whether saving this draft is taking the setting's own value away, so that what is above applies again. */
export function clearsSetting(key: string, value: unknown): boolean {
  return key === SUMMARIZER_SETTING && noSummarizer(value);
}
