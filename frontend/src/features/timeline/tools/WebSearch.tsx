/** Search query and result links.
 *
 * The results of a search of the web, each a title and an address that opens in a tab of its own.
 */
import { ExternalLink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import type { Item } from '@/generated/api-types';
import { isSafeHref } from '@/lib/markdown';

type SearchItem = Extract<Item, { kind: 'tool_call' }>;

export function WebSearchDisplay({ item }: { item: SearchItem }) {
  const { t } = useTranslation();
  const tool = item.payload.tool;
  if (tool.kind !== 'web_search') return null;
  if (tool.results.length === 0) return <EmptyState compact title={t('timeline.web.noResults')} />;
  return (
    <ul className="flex flex-col gap-1">
      {tool.results.map((result) => (
        <li key={result.url} className="flex min-w-0 flex-col text-sm">
          {isSafeHref(result.url) ? (
            <a href={result.url} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1 text-accent-text underline underline-offset-2">
              <span className="truncate">{result.title === '' ? result.url : result.title}</span>
              <ExternalLink aria-hidden />
              <span className="sr-only">{t('timeline.web.newTab')}</span>
            </a>
          ) : (
            <span className="text-text">{result.title}</span>
          )}
          <span className="truncate text-xs text-text-subtle">{result.url}</span>
        </li>
      ))}
    </ul>
  );
}
