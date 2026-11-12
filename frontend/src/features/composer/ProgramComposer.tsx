/** A permanent built-in thread uses the shared question composer and its real native read scope. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { askReader, sendAsk } from '@/api/ask';
import { client } from '@/api/client';
import { captureThreadExecution, threadAct } from '@/api/threadRead';
import { fetchThreadSync, interruptTurn } from '@/api/threads';
import type { AskModel, Thread } from '@/generated/api-types';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { currentTurnOver, runningTurn, useThreadPart } from '@/realtime/stores/thread';
import { holds } from '@/lib/permissions';
import { AskComposer } from './AskComposer';

export function ProgramComposer({ thread, onSent }: { thread: Thread; onSent?: () => void }) {
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion}`);
  const { t } = useTranslation();
  if (thread.program?.id !== 'orch') return null;
  return <div data-program-composer><p className="px-3 pt-2 text-sm text-text-muted" data-composer-control="program">{t('ask.threadProgram')}</p>
    <Question key={`${reader}:${thread.id}`} thread={thread} root={thread.program.root_org_id} {...(onSent === undefined ? {} : { onSent })} /></div>;
}

function Question({ thread, root, onSent }: { thread: Thread; root: string; onSent?: () => void }) {
  const read = useThreadReadRef(thread.id);
  const turn = useThreadPart(thread.id, runningTurn);
  const [choice, setChoice] = useState<AskModel | null | undefined>(undefined);
  const model = choice === undefined ? thread.program_model ?? null : choice;
  const over = useThreadPart(thread.id, currentTurnOver);
  const active = turn !== null || !over && (thread.state === 'running' || thread.state === 'waiting_approval');
  const turnId = turn?.id ?? thread.current_turn_id;
  const mayDrive = holds(thread.access, 'thread.interact') && !thread.archived;
  const writable = mayDrive && thread.lock === null;
  return <AskComposer scope={thread.id} root={root} draftKey={thread.id} conversationId={thread.id}
    model={model} onModel={setChoice} active={active} disabled={!writable || thread.state === 'starting'}
    canStop={mayDrive && turnId !== null} ensure={() => new Promise<string>((resolve) => { captureThreadExecution(read); resolve(thread.id); })}
    send={async (content, clientId) => {
      const current = askReader();
      await threadAct(read, () => sendAsk(thread.id, { client_id: clientId, content, context: null, model }, choice === undefined));
      current();
      await fetchThreadSync(read, null);
      current();
      setChoice(undefined);
      onSent?.();
    }}
    stop={async () => {
      if (turnId === null) return;
      const current = askReader();
      await interruptTurn(read, turnId);
      current();
      await fetchThreadSync(read, null);
      current();
    }} />;
}
