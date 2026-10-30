/** A search of files by content or by name: what was looked for, where, and the matches as links.
 *
 * What a search of files found: what was looked for, where, and the hits by file with the line and the
 * text of it. Where the list was cut it says how many more there were.
 */
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { FileLink } from '@/features/files/FileLink';
import type { Item } from '@/generated/api-types';

type SearchItem = Extract<Item, { kind: 'tool_call' }>;

export function FileSearchDisplay({ item }: { item: SearchItem }) {
  const { t } = useTranslation();
  const tool = item.payload.tool;
  if (tool.kind !== 'file_search') return null;
  const hidden = tool.total === null ? 0 : Math.max(0, tool.total - tool.matches.length);
  return (
    <div className="flex flex-col gap-2">
      <p className="text-sm text-text-muted">
        {tool.mode === 'name' ? t('timeline.search.byName', { query: tool.query }) : t('timeline.search.byContent', { query: tool.query })}
        {tool.path !== null && tool.path !== '' && ` ${t('timeline.search.under', { path: tool.path })}`}
      </p>
      {tool.matches.length === 0 ? (
        <EmptyState compact title={t('timeline.search.none')} />
      ) : (
        <ul className="flex max-h-(--output-height) flex-col overflow-auto rounded-lg border border-border bg-bg-inset font-mono text-sm">
          {tool.matches.map((match, index) => (
            <li key={`${match.path}.${match.line ?? 0}.${index}`} className="flex gap-2 px-3 py-1">
              <span className="shrink-0 text-text">
                <FileLink path={match.path} line={match.line} />
                {match.line !== null && <span className="text-text-subtle">:{match.line}</span>}
              </span>
              {match.text !== null && <span className="min-w-0 truncate text-text-muted">{match.text.trim()}</span>}
            </li>
          ))}
        </ul>
      )}
      {(tool.truncated || hidden > 0) && <p className="text-xs text-text-subtle">{hidden > 0 ? t('timeline.search.more', { count: hidden }) : t('timeline.search.cut')}</p>}
    </div>
  );
}
