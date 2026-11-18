/** One visible conversation owns one snapshot and one resumable event subscription. */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { askKey, askQuery, streamAsk } from '@/api/ask';
import { queryClient } from '@/api/query';
import type { AskGetReply } from '@/generated/api-types';
import { acceptsAskEvent, applyAskEvent } from './events';
import { NetworkError } from '@/lib/requests';

export function useConversation(id: string | null, visible: boolean, inspect = false) {
  const { t } = useTranslation();
  const [presentationId, setPresentationId] = useState(id);
  const [streamError, setStreamError] = useState<unknown>(null);
  const [notice, setNotice] = useState('');
  const [deleted, setDeleted] = useState(false);
  const [tools, setTools] = useState<Record<string, string>>({});
  const [retry, setRetry] = useState(0);
  const sameConversation = presentationId === id;
  const ended = sameConversation && deleted;
  const read = useQuery({ ...askQuery(id ?? '', inspect), enabled: id !== null && visible && !ended });
  const { refetch } = read;
  const readRefused = read.error instanceof ApiError && [401, 403, 404, 410].includes(read.error.status);
  const streamRefused = sameConversation && streamError instanceof ApiError && [401, 403, 404, 410].includes(streamError.status);
  const refused = readRefused || streamRefused;
  // Adjust this presentation before committing a frame for another conversation.
  if (presentationId !== id) { setPresentationId(id); setNotice(''); setDeleted(false); setStreamError(null); setTools({}); }
  const snapshotState = read.data?.conversation.state;
  const [toolsState, rememberToolsState] = useState(snapshotState);
  if (toolsState !== snapshotState) { rememberToolsState(snapshotState); if (snapshotState !== 'running') setTools({}); }
  const ready = read.data !== undefined && !refused;
  useEffect(() => {
    if (id === null || !visible || !ready || ended) return;
    const abort = new AbortController();
    let stopped = false;
    let attempts = 0;
    // Native refreshes contain messages, but cannot acknowledge presentation-only stream notices.
    let cursor: number | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const resume = async () => {
      try {
        const current = queryClient.getQueryData<AskGetReply>(askKey(id, inspect));
        if (current === undefined) return;
        cursor ??= current.conversation.seq;
        await streamAsk(id, cursor, (event) => {
          if (abort.signal.aborted || stopped) return;
          const current = queryClient.getQueryData<AskGetReply>(askKey(id, inspect));
          if (current === undefined || event.conversation_id !== id || !Number.isSafeInteger(event.seq) || event.seq <= (cursor ?? current.conversation.seq)) return;
          cursor = event.seq;
          attempts = 0;
          if (event.data.kind === 'error' && event.data.code === 'ask_context_too_long' && 'model' in event.data && typeof event.data.model === 'string') setNotice(t('ask.contextTooLong', { model: event.data.model }));
          else if (event.data.kind === 'error' && event.data.code === 'ask_turn_too_long' && 'model' in event.data && typeof event.data.model === 'string') setNotice(t('ask.turnTooLong', { model: event.data.model }));
          else if (event.data.kind === 'notice' || event.data.kind === 'error') setNotice(event.data.message);
          if (!acceptsAskEvent(current, event)) return;
          if (event.data.kind === 'deleted') { stopped = true; setDeleted(true); queryClient.removeQueries({ queryKey: askKey(id, inspect), exact: true }); return; }
          if (event.data.kind === 'tool') {
            const tool = event.data;
            setTools((before) => { const { [tool.call_id]: _previous, ...rest } = before; return tool.complete ? rest : { ...rest, [tool.call_id]: tool.name }; });
          }
          const next = applyAskEvent(current, event);
          if (next === null) { stopped = true; void refetch().finally(() => { if (!abort.signal.aborted) setRetry((value) => value + 1); }); }
          else { queryClient.setQueryData(askKey(id, inspect), next); setStreamError(null); }
        }, abort.signal);
      } catch (error) {
        if (abort.signal.aborted) return;
        setStreamError(error);
        if (error instanceof ApiError && [401, 403, 404, 410].includes(error.status)) {
          stopped = true;
          queryClient.removeQueries({ queryKey: askKey(id, inspect), exact: true });
        }
      }
      // The shared read policy is three retries after 1, 2 and 4 seconds, then an explicit Retry.
      if (!abort.signal.aborted && !stopped) {
        if (attempts < 3) timer = setTimeout(() => void resume(), 1000 * 2 ** attempts++);
        else setStreamError((previous: unknown) => previous ?? new NetworkError('failed'));
      }
    };
    void resume();
    return () => { abort.abort(); if (timer !== undefined) clearTimeout(timer); };
  }, [id, visible, ready, ended, inspect, retry, refetch, t]);
  return { ...read, data: ended || refused ? undefined : read.data,
    streamError: sameConversation && read.error === null ? streamError : null,
    notice: sameConversation && !refused ? notice : '', deleted: ended, tools: sameConversation && !ended && !refused ? tools : {},
    reconnect: () => { setStreamError(null); setRetry((value) => value + 1); void refetch().catch(() => undefined); } };
}
