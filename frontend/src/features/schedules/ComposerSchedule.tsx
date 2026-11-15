/** Capture a complete composer draft and clear only its unchanged accepted snapshot. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { type ReactNode, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { captureThreadRead, isCurrentThreadRead } from '@/api/threadRead';
import { draftQuery } from '@/api/threads';
import type { ContentPart, Draft, Thread } from '@/generated/api-types';
import { composerWriter, useComposerStore } from '@/features/composer/Draft';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { ScheduleEditor, type AuthoredScheduleDraft } from './Editor';

interface Frozen { threadId: string; draft: AuthoredScheduleDraft; planning: boolean | undefined; text: string; parts: ContentPart[]; read: ReturnType<typeof captureThreadRead>; native: Draft['native_action'] }
export function useScheduleComposer(thread: Thread, enabled: boolean, unavailable: string | false, choices: Pick<AuthoredScheduleDraft, 'plan' | 'delivery'>): { open: () => void; reason: string | false; dialog: ReactNode } {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const read = useThreadReadRef(thread.id);
  const stored = useQuery({ ...draftQuery(thread.id), enabled });
  const [frozen, setFrozen] = useState<Frozen | null>(null);
  const currentChoices = useRef(choices);
  useEffect(() => { currentChoices.current = choices; }, [choices]);
  const reason = unavailable || (!stored.isSuccess ? t('sendLater.draftLoading') : (stored.data?.review_comments.length ?? 0) > 0 ? t('sendLater.reviewUnsupported') : false);
  const open = () => {
    if (reason !== false) return;
    const current = useComposerStore.getState();
    const text = current.text[thread.id] ?? '', parts = current.parts[thread.id] ?? [];
    const native = Object.hasOwn(current.nativeSelection, thread.id) ? current.nativeSelection[thread.id] ?? null : stored.data?.native_action ?? null;
    let captured: ReturnType<typeof captureThreadRead>;
    try { captured = captureThreadRead(read); } catch { return; }
    const content = [...(text === '' ? [] : [{ type: 'text' as const, text }]), ...parts];
    setFrozen({ threadId: thread.id, draft: structuredClone({ content, nativeAction: native, ...choices }), planning: current.planning[thread.id], text, parts: structuredClone(parts), read: captured, native });
  };
  const current = frozen !== null && isCurrentThreadRead(frozen.read) && frozen.threadId === thread.id;
  const accepted = () => {
    if (frozen === null) return;
    const same = () => isCurrentThreadRead(frozen.read)
      && useComposerStore.getState().planning[thread.id] === frozen.planning
      && currentChoices.current.plan === frozen.draft.plan && currentChoices.current.delivery === frozen.draft.delivery;
    const latest = cache.getQueryData<Draft | null>(draftQuery(thread.id).queryKey);
    const selection = useComposerStore.getState().nativeSelection;
    const native = Object.hasOwn(selection, thread.id) ? selection[thread.id] ?? null : latest?.native_action ?? null;
    if (!same() || JSON.stringify(native) !== JSON.stringify(frozen.native)) return;
    composerWriter.clearScheduledDraft(thread.id, frozen.text, frozen.parts, same);
  };
  return { open, reason, dialog: current ? <ScheduleEditor topic={thread.topic_id} thread={thread} draft={frozen.draft} onClose={() => setFrozen(null)} onAccepted={accepted} /> : null };
}
