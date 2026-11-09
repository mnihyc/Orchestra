/** Thread Duplicate remains its native fork, with the existing agent-access decision. */
import { useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { ForkProgram } from '@/features/ask/Keep';
import { openChange } from '@/features/change/target';
import { ImmediateForkChoice } from '@/features/thread/ImmediateFork';
import { threadCapabilities } from '@/features/thread/cached';
import { useForkNow } from '@/features/thread/forkNow';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import type { DuplicateTarget } from './target';

export function ThreadDuplicate({ target, quick, onClose }: { target: DuplicateTarget; quick: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const read = useQuery(nodeQuery(target.id));
  const fork = useForkNow(target.id);
  const navigate = useNavigate();
  const [reader] = useState(libraryReader);
  const handedOff = useRef(false);
  const thread = read.data?.kind === 'thread' ? read.data : null;
  const allowed = thread !== null && !thread.archived && thread.lock === null && threadCapabilities(thread)?.fork === true;
  useEffect(() => {
    if (!quick && allowed && thread.program?.id !== 'orch' && !handedOff.current && sameLibraryReader(reader)) {
      handedOff.current = true;
      onClose();
      openChange({ kind: 'thread', id: target.id, name: target.name }, { kind: 'fork' });
    }
  }, [quick, allowed, thread?.program?.id, target.id, target.name, onClose, reader]);
  if (!sameLibraryReader(reader)) return null;
  if (thread !== null && allowed) {
    if (thread.program?.id === 'orch') return <ForkProgram thread={thread} through={undefined} onClose={onClose} onKept={(id) => {
      if (sameLibraryReader(reader)) { onClose(); void navigate({ to: '/thread/$threadId', params: { threadId: id } }); }
    }} />;
    if (quick) return <ImmediateForkChoice thread={thread} through={undefined} onClose={onClose} onConfirm={(choice) => {
      if (sameLibraryReader(reader)) { fork.start(undefined, choice); onClose(); }
    }} />;
    return null;
  }
  return <Dialog open title={t('duplicate.title', { name: target.name })} onOpenChange={(open) => { if (!open) onClose(); }} footer={<Button onClick={onClose}>{t('common.cancel')}</Button>}>
    {read.isPending ? <SkeletonRows read={read} rows={2} /> : read.isError ? <ErrorState compact error={read.error} onRetry={() => void read.refetch()} />
      : <p className="text-sm text-text-muted">{t(thread?.lock === null ? 'duplicate.readRequired' : 'threadMove.jobMustEnd')}</p>}
  </Dialog>;
}
