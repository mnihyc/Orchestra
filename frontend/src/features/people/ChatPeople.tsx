/** Standing reading access and each currently shared agent's local posting control. */
import { Play, Square } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { setAgentPosting } from '@/api/people';
import { holderName } from '@/features/access/holders';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import type { Chat as PeopleChat } from '@/generated/api-types';

export function ChatPeople({ chat, me }: { chat: PeopleChat; me: string | null }) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const holders = chat.inspection_holders;
  const agents = (chat.agents ?? []).filter((agent) => agent.readable);
  const manage = chat.can.moderate || (chat.kind === 'group' && chat.created_by?.id === me);
  const posting = async (threadId: string, stopped: boolean) => {
    if (pending !== null) return;
    setPending(threadId); setError(null);
    try { await setAgentPosting(chat.id, threadId, stopped); }
    catch (failure) { setError(failure); }
    finally { setPending(null); }
  };
  if (chat.kind === 'node' && agents.length === 0) return null;
  return <div className="flex shrink-0 flex-col gap-1 border-b border-border px-3 py-1 text-xs text-text-muted">
    {chat.kind !== 'node' && <p>{holders === null || holders === undefined ? t('people.readersUnknown') : holders.length === 0 ? t('people.readersPeople') : t('people.readersInspection', { holders: holders.map((holder) => holderName(t, holder, '',false,label)).join(', ') })}</p>}
    {agents.map((agent) => <div key={agent.thread.id} className="flex flex-wrap items-center gap-2">
      <span>{t(agent.writable && !agent.stopped ? 'people.agentPost' : 'people.agentRead', { agent: agent.agent, thread: agent.thread.redacted ? t('people.privateThread') : agent.thread.name ?? t('people.privateThread') })}</span>
      {agent.stopped && <span className="rounded-full border border-border px-2 py-1">{t('people.agentStopped')}</span>}
      {agent.next_allowed_at !== null && <span>{t('people.nextPost', { time: new Date(agent.next_allowed_at).toLocaleString() })}</span>}
      {manage && <Button size="sm" variant="ghost" pending={pending === agent.thread.id} icon={agent.stopped ? <Play aria-hidden /> : <Square aria-hidden />} onClick={() => void posting(agent.thread.id, !agent.stopped)}>{t(agent.stopped ? 'people.allowPosting' : 'people.stopPosting')}</Button>}
    </div>)}
    {error !== null && <ErrorAlert error={error} />}
  </div>;
}
