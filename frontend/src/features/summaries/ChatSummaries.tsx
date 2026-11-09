/** Opening unread bounds belong to this reader; a generated subject never replaces a written group name. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { resourceSummaryQuery, summaryReaderKey, type ResourceSummaryRange } from '@/api/summaries';
import { Disclosure } from '@/ui/Disclosure';
import { ResourceSummary } from './ResourceSummary';

export function ChatSummaries({ id, range, named, shown }: { id: string; range: ResourceSummaryRange; named: boolean; shown: boolean }) {
  const { t } = useTranslation();
  useStore(client.session, summaryReaderKey);
  const title = useQuery({ ...resourceSummaryQuery({ kind: 'chat_title', id }), enabled: shown });
  return <div className="flex min-w-0 shrink-0 flex-col gap-2 border-b border-border px-3 py-2" data-chat-summaries>
    {!named && !title.isError && title.data?.text != null && <p className="m-0 text-sm font-medium text-text wrap-anywhere" data-chat-subject>{title.data.text}</p>}
    <ResourceSummary target={{ kind: 'chat', id }} reference={`/chat/${id}`} range={range} enabled={shown} />
    <Disclosure title={t('summaryOptions.fields.chat_title')}>
      <div className="py-2"><ResourceSummary target={{ kind: 'chat_title', id }} reference={`/chat/${id}`} enabled={shown} /></div>
    </Disclosure>
  </div>;
}
