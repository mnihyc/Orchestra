/** Public build-time guide text is editable Ask context; references still grant no authority. */
import { openAskWithDraft } from '@/features/ask/open';

export interface HelpAskEntry {
  readonly article: string;
  readonly heading?: string | null;
  readonly title: string;
  readonly summary?: string;
}
export function helpAskReference(entry: HelpAskEntry): string {
  return `/help/${encodeURIComponent(entry.article)}${entry.heading == null ? '' : `#${encodeURIComponent(entry.heading)}`}`;
}
/** Freeze the visible public selection, including its authored language, at the explicit click. */
export function helpAskText(label: string, entries: readonly HelpAskEntry[]): string {
  return [label, ...entries.map((entry) => [entry.title, helpAskReference(entry),
    ...(entry.summary === undefined ? [] : [entry.summary.split('\n').map((line) => `> ${line}`).join('\n')]),
  ].join('\n'))].join('\n\n');
}
export function askAboutHelp(label: string, entries: readonly HelpAskEntry[]): void {
  if (entries.length > 0) openAskWithDraft({ text: helpAskText(label, entries) });
}
