/** Shared HistoryLines component (access): the lines of a thing's history, who did what and when, rendered from the audit catalogue.
 *
 * Newest first, one line each: who did it as an avatar, what was done in one sentence, and when. A
 * line holds no content of the thing, only the sentence; whatever it points at is opened from it. The
 * list is read a page at a time, and an agent that did something is written as its thread's agent.
 */
import { Bot } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { ReactNode } from 'react';

import type { HistoryLine } from '@/generated/api-types';
import { historySentence } from '@/features/inspector/historySentence';
import { Avatar } from '@/ui/Avatar';
import { cx } from '@/ui/cx';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { UploadHistory } from '@/features/files/UploadHistory';

export interface HistoryLinesProps {
  lines: readonly HistoryLine[];
  hasMore?: boolean;
  loading?: boolean;
  /** What the last attempt at the next page failed with. */
  error?: unknown;
  onMore?: () => void;
  className?: string;
  /** Links or acts supplied by the screen for pointers the line carries. */
  renderDetails?: (line: HistoryLine) => ReactNode;
}

export function HistoryLines({ lines, hasMore = false, loading = false, error, onMore, className, renderDetails }: HistoryLinesProps) {
  const { t } = useTranslation();
  return (
    <div className={className}>
      <ol aria-label={t('access.history.label')} className="flex flex-col">
        {lines.map((line) => (
          <li key={line.id} className="flex items-start gap-2 border-b border-border py-2 last:border-b-0">
            {line.upload !== undefined && line.upload !== null ? null : line.actor === null ? (
              <Avatar text="·" color={0} size={24} kind="org" />
            ) : 'agent' in line.actor ? (
              <Avatar text="" color={0} size={24} kind="agent" mark={<Bot size={14} aria-hidden />} />
            ) : (
              <Avatar text={line.actor.avatar.text} color={line.actor.avatar.color} size={24} kind={line.actor.kind === 'service' ? 'service' : 'human'} />
            )}
            <div className="flex min-w-0 flex-1 flex-col">
              {line.upload !== undefined && line.upload !== null ? <UploadHistory line={line} /> : <span className="text-base wrap-anywhere text-text">{historySentence(line, t)}</span>}
              <RelativeTime at={line.at} className={cx('text-sm text-text-muted')} />
              {renderDetails?.(line)}
            </div>
          </li>
        ))}
      </ol>
      {onMore !== undefined && <Pagination hasMore={hasMore} loading={loading} error={error} onMore={onMore} />}
    </div>
  );
}
