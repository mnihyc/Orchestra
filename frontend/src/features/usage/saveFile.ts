/** Hands a file the page holds to the person: a name for it, and a click on a link to it that the browser takes as a save, and the names a usage file is given.
 *
 * What is asked of the server as text (the series, the records) is collected whole and offered when it
 * is complete; the browser keeps the bytes in memory until the save is done, which is why a file of
 * records is cut at a most.
 */
import type { Span, UsageScope } from '@/features/usage/search';

/** How long the address of a file made here stays good: long enough for the browser to begin the save. */
const RELEASE_AFTER_MS = 60_000;

/** Offers the text as a file with this name. */
export function saveFile(text: string, name: string, type = 'text/csv'): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), RELEASE_AFTER_MS);
}

/** What a usage file is called: whose use, which days, and what it holds. */
export function usageFileName(scope: UsageScope, span: Span, what: 'series' | 'records'): string {
  const whose = scope.kind === 'me' ? 'me' : scope.kind === 'node' ? 'place' : 'provider';
  return `orch-usage-${whose}-${span.from}-to-${span.to}-${what}.csv`;
}
