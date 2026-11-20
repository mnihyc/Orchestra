/** Opening unread bounds belong to this reader; a generated subject never replaces a written group name. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { useState } from 'react';
import { client } from '@/api/client';
import { resourceSummaryQuery, summaryReaderKey, type ResourceSummaryRange } from '@/api/summaries';
import { Disclosure } from '@/ui/Disclosure';
import { ResourceSummary } from './ResourceSummary';

interface ChatSummariesProps {
  id: string;
  range: ResourceSummaryRange;
  named: boolean;
  shown: boolean;
  hasMessages: boolean;
}

export function ChatSummaries(props: ChatSummariesProps) {
  if (!props.hasMessages) return null;
  return <RecordedChatSummaries key={props.id} {...props} />;
}

function RecordedChatSummaries({ id, range, named, shown }: ChatSummariesProps) {
  const { t } = useTranslation();
  const [titleOpen, setTitleOpen] = useState(false);
  useStore(client.session, summaryReaderKey);
  const title = useQuery({ ...resourceSummaryQuery({ kind: 'chat_title', id }), enabled: shown && !named });
  return <div className="flex min-w-0 shrink-0 flex-col gap-2 border-b border-border px-3 py-2" data-chat-summaries>
    {!named && !title.isError && title.data?.text !== undefined && title.data.text !== null && <p className="m-0 text-sm font-medium text-text wrap-anywhere" data-chat-subject>{title.data.text}</p>}
    {range.after < range.through && <ResourceSummary target={{ kind: 'chat', id }} reference={`/chat/${id}`} range={range} enabled={shown} />}
    <Disclosure title={t('summaryOptions.fields.chat_title')} onOpenChange={setTitleOpen}>
      <div className="py-2"><ResourceSummary target={{ kind: 'chat_title', id }} reference={`/chat/${id}`} enabled={shown && titleOpen} /></div>
    </Disclosure>
  </div>;
}
