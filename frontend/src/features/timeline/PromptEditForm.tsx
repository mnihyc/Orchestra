/** Delivered prompt edits use the same text-part rules and controls as pending-input editing. */
import { type ReactNode, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { editableWords, withEditedWords } from '@/features/composer/queue';
import type { ContentPart, ContinuationActiveTurn } from '@/generated/api-types';
import { isComposing } from '@/lib/shortcuts';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Textarea } from '@/ui/Textarea';
import { AttachmentChips, UploadChips } from '@/features/composer/Attachments';
import { usePastedText } from '@/features/composer/usePastedText';
import { useTimelineContext } from '@/features/timeline/context';
import { pasteKey } from '@/lib/pastedText';
import { releaseUpload } from '@/lib/uploads';

export interface PromptEditFormProps {
  parts: readonly ContentPart[];
  activeTurnId: string | null;
  pending?: boolean;
  retryOnly?: boolean;
  unavailable?: string | null;
  error?: unknown;
  children?: ReactNode;
  onCancel: () => void;
  onSubmit: (content: ContentPart[], activeTurn: ContinuationActiveTurn | undefined) => void;
}

export function PromptEditForm({ parts, activeTurnId, pending = false, retryOnly = false, unavailable = null, error,
  children, onCancel, onSubmit }: PromptEditFormProps) {
  const { t } = useTranslation();
  const [words, setWords] = useState(() => editableWords(parts));
  const [editedParts, setEditedParts] = useState(() => [...parts]);
  const box = useRef<HTMLTextAreaElement>(null);
  const { threadId } = useTimelineContext();
  const pastes = usePastedText({ kind: 'thread', id: threadId, box, maxLines: 4, text: words, setText: setWords, temporary: true,
    add: (part) => setEditedParts((before) => [...before, part]),
    replace: (original, part) => setEditedParts((before) => before.map((held) => held.type === 'pasted_text' && pasteKey(held) === pasteKey(original) ? part : held)),
    remove: (index) => setEditedParts((before) => before.filter((_, at) => at !== index)),
  });
  const content = withEditedWords(editedParts, words);
  const blocked = pending || pastes.blocked || unavailable !== null || content.length === 0;
  const cancel = () => {
    for (const part of editedParts) if (part.type === 'pasted_text') {
      pastes.cancel(part);
      if (!parts.some((original) => original.type === 'pasted_text' && pasteKey(original) === pasteKey(part))) void releaseUpload(part.blob, { thread: threadId, path: part.path }).catch(() => undefined);
    }
    onCancel();
  };
  const send = (action: ContinuationActiveTurn['action']) => {
    if (blocked) return;
    onSubmit(content, activeTurnId === null ? undefined : { turn_id: activeTurnId, action });
  };
  return <form className="flex min-w-0 flex-col gap-2" onSubmit={(event) => { event.preventDefault(); send('wait'); }}>
    <Textarea ref={box} label={t('messageVersions.edit')} rows={4} value={words} autoFocus disabled={pending || retryOnly}
      onChange={(event) => setWords(event.target.value)} aria-keyshortcuts="Control+Enter Meta+Enter Escape"
      onKeyDown={(event) => {
        if (isComposing(event.nativeEvent)) return;
        if (event.key === 'Escape' && !pending) { event.preventDefault(); event.stopPropagation(); cancel(); }
        else if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey) {
          event.preventDefault(); event.stopPropagation(); event.currentTarget.form?.requestSubmit();
        }
      }} />
    <UploadChips queue={pastes.queue} />
    <AttachmentChips parts={editedParts} readPaste={pastes.read} pastedIdentity={pastes.identity} onPasteAsText={pastes.asText} onPasteDirty={pastes.markDirty} editText={pastes.editText} editing={pastes.editing} pasteQueue={pastes.queue} onPasteDelivery={pending || retryOnly ? undefined : pastes.delivery}
      onEditPaste={pending || retryOnly ? undefined : (index, text) => { const part = editedParts[index]; if (part?.type === 'pasted_text') pastes.edit(part, text); }} />
    {children}
    <p className="text-xs text-text-muted">{unavailable ?? t('messageVersions.editAbout')}</p>
    {error !== undefined && error !== null && <ErrorAlert error={error} />}
    <div className="flex flex-wrap items-center gap-2">
      <Button type="submit" size="sm" variant="primary" pending={pending} disabled={blocked}>
        {retryOnly ? t('common.tryAgain') : activeTurnId === null ? t('messageVersions.send') : t('messageVersions.wait')}
      </Button>
      {activeTurnId !== null && !retryOnly && <Button type="button" size="sm" variant="secondary" disabled={blocked} onClick={() => send('interrupt')}>
        {t('messageVersions.interrupt')}
      </Button>}
      <Button type="button" size="sm" variant="ghost" disabled={pending} onClick={cancel}>{t('common.cancel')}</Button>
    </div>
  </form>;
}
