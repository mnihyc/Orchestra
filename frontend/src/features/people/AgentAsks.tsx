/** The actual ordinary inputs returned by the guarded, rolled-back agent ask preview. */
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { AccessRequest } from '@/features/access/AccessRequest';

import { ForwardedBlocks } from '@/features/forward/ForwardedBlock';
import { placePath } from '@/features/navigation/places';
import { Body } from '@/features/chat/Body';
import type { ChatAgentAskPreview, ChatAskedAgent } from '@/generated/api-types';
import { isForwarded } from '@/lib/content';
import { AgentChip } from '@/ui/AgentChip';
import { ErrorAlert } from '@/ui/ErrorState';

export function AgentAskError({ error }: { error: unknown }) {
  const denied = error instanceof ApiError ? error.extra('authz.denied') : undefined;
  const missing = denied?.missing ?? [];
  const object = missing[0]?.object.id;
  return <div className="flex flex-col gap-2"><ErrorAlert error={error} />{denied?.requestable === true && object !== undefined && missing.some((entry) => entry.closed === null) && <AccessRequest missing={missing} objectId={object} fixed />}</div>;
}

export function AgentAsks({ chatId, asks }: { chatId: string; asks: ChatAgentAskPreview[] }) {
  const { t } = useTranslation();
  return <div className="flex max-h-[40dvh] flex-col gap-3 overflow-y-auto rounded-lg border border-border bg-bg-subtle p-3">
    {asks.map((ask) => <section key={ask.thread.id} className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2 text-sm"><AgentChip name={ask.agent ?? t('access.agents.dialog.noAgent')} thread={ask.thread} /><span>{ask.thread.name}</span><span className="text-text-muted">{ask.model ?? t('people.forwardNoModel')}{ask.effort === null ? '' : ` · ${ask.effort}`}</span></div>
      <p className="text-xs text-text-muted">{t('people.forwardQueuedUse', { thread:ask.thread.name })}</p>
      {ask.input.content.filter((part) => part.type === 'text').map((part, index) => <Body key={index} text={part.text} />)}
      <ForwardedBlocks chatId={chatId} parts={ask.input.content.filter(isForwarded)} />
    </section>)}
  </div>;
}

/** Only the thread's current visible reference is linked; a revoked thread keeps no displayed name. */
export function AskedAgents({ asks }: { asks: ChatAskedAgent[] }) {
  const { t } = useTranslation();
  return <div className="flex flex-wrap gap-2 text-xs text-text-muted">{asks.map((ask) => ask.thread.redacted || ask.thread.name === null
    ? <span key={ask.input_id}>{t('people.askedThread', { thread:t('people.privateThread') })}</span>
    : <a key={ask.input_id} href={placePath({ id:ask.thread.id, kind:'thread' })} className="text-accent-text hover:underline">{t('people.askedThread', { thread:ask.thread.name })}</a>)}</div>;
}
