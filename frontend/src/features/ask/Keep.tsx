/** Keeping creates one Private permanent thread, only at an authorized place of the original root. */
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { askReader, keepAsk } from '@/api/ask';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import { threadCapabilities } from '@/features/thread/cached';
import type { AskConversation, Thread } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';

export function KeepAsk({ conversation, onKept, onClose }: { conversation: AskConversation; onKept: (thread: string) => void; onClose: () => void }) {
  const { t } = useTranslation();
  return <PreserveProgram id={conversation.id} root={conversation.root_org_id} allowed={conversation.can.keep}
    title={t('ask.keep')} confirm={t('ask.keep')} defaultPlace={t('ask.defaultDestination')} onKept={onKept} onClose={onClose} />;
}

/** A permanent source has native thread facts, with no temporary retention or owner header. */
export function ForkProgram({ thread, through, onKept, onClose }: { thread: Thread; through: number | undefined; onKept: (thread: string) => void; onClose: () => void }) {
  const { t } = useTranslation();
  if (thread.program?.id !== 'orch') return null;
  return <PreserveProgram id={thread.id} root={thread.program.root_org_id}
    allowed={threadCapabilities(thread)?.fork === true && !thread.archived && thread.lock === null}
    title={t('ask.forkTitle', { name: thread.name })} confirm={t('change.menu.forkNow')}
    defaultPlace={t('ask.forkDefaultDestination')} through={through} onKept={onKept} onClose={onClose} />;
}

function PreserveProgram({ id, root, allowed, title, confirm, defaultPlace, through, onKept, onClose }: {
  id: string; root: string; allowed: boolean; title: string; confirm: string; defaultPlace: string;
  through?: number | undefined; onKept: (thread: string) => void; onClose: () => void;
}) {
  const { t } = useTranslation();
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const [pending, setPending] = useState(false), [failure, setFailure] = useState<unknown>(null);
  const busy = useRef(false);
  const intent = useRef<{ fingerprint: string; key: string } | null>(null);
  const keep = async () => {
    if (busy.current || !allowed) return;
    busy.current = true; setPending(true); setFailure(null);
    const current = askReader();
    const body = { topic_id: place?.id ?? null, project_id: null, title: null };
    const fingerprint = JSON.stringify([id, through, body]);
    const attempt = intent.current?.fingerprint === fingerprint ? intent.current : { fingerprint, key: newActKey() };
    intent.current = attempt;
    try { const reply = await keepAsk(id, body, { key: attempt.key, ...(through === undefined ? {} : { toTurn: through }) }); current(); onKept(reply.thread_id); }
    catch (error) { setFailure(error); } finally { busy.current = false; setPending(false); }
  };
  return <Dialog open onOpenChange={(open) => { if (!open && !pending) onClose(); }} dismissible={!pending} title={title}>
    <div className="flex flex-col gap-3"><p className="text-sm text-text-muted">{t('ask.privateThread')}</p>
      {through !== undefined && <p className="text-sm text-text-muted">{t('agentAccess.forkThrough', { turn: through })}</p>}
      <PlacePicker label={t('ask.destination')} rootOrgId={root} only="topic" value={place} onChange={setPlace} disabled={pending} />
      {place === null && <span className="text-sm text-text-muted">{defaultPlace}</span>}
      {failure !== null && <ErrorAlert error={failure} />}
      <div className="flex justify-end gap-2"><Button onClick={onClose} disabled={pending}>{t('common.cancel')}</Button><Button variant="primary" pending={pending} disabled={!allowed} onClick={() => void keep()}>{confirm}</Button></div>
    </div>
  </Dialog>;
}
