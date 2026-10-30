/** Upload, paste and file/image attachments.
 *
 * Files selected, dropped or pasted use the shared resumable uploader. Pending and failed files
 * remain visible until attached or removed; ready parts survive in drafts and cannot be sent early.
 */
import { FileText, Paperclip, Plus, RotateCcw, X } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { lazy, Suspense, useRef, useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useTranslation } from 'react-i18next';

import { IconButton } from '@/ui/IconButton';
import { FileAttachment } from '@/features/files/FileChip';
import { FileGuideHint } from '@/features/files/FileGuideHint';
import { blobSource } from '@/features/files/source';
import { FilePathAttachment } from '@/features/files/FileLink';
import { Menu } from '@/ui/Menu';
import type { ChatPart, ContentPart } from '@/generated/api-types';
import type { QueuedUpload } from '@/lib/useUploads';
import { ByteSize } from '@/ui/ByteSize';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { formatBytes, formatNumber } from '@/lib/format';
import { useAppearance } from '@/lib/appearance';
import { pasteDisplayKey, pasteKey, rememberedPaste, type PastedTextPart } from '@/lib/pastedText';
import { SegmentedControl } from '@/ui/SegmentedControl';
import type { CodeViewHandle } from '@/ui/CodeView';
import { useViewShown } from '@/features/workbench/viewOpening';
import { Button } from '@/ui/Button';
import { SkeletonRows } from '@/ui/Skeleton';
import { cx } from '@/ui/cx';
import { UploadHelp } from '@/features/composer/UploadHelp';
import { JobActivityLine } from '@/features/change/Progress';
export { carriesFiles } from '@/lib/drop';

export function AttachButton({ onPress, label, disabled = false, onFile, onDocument, fileReason = false, help = 'message-files' }: { onPress: () => void; label?: string; disabled?: boolean; onFile?: () => void; onDocument?: () => void; fileReason?: string | false; help?: 'message-files' | 'documents' }) {
  const { t } = useTranslation();
  const name = label ?? t(onFile !== undefined && onDocument !== undefined ? 'composer.plus.label' : 'composer.attach');
  const button = onFile !== undefined && onDocument !== undefined ? <Menu label={name} side="top" trigger={<IconButton size="sm" label={name} icon={<Plus aria-hidden />} disabled={disabled} />} entries={[
    { id: 'upload', label: t('composer.plus.upload'), onSelect: onPress },
    { id: 'file', label: t('composer.plus.file'), disabled: fileReason, onSelect: onFile },
    { id: 'document', label: t('composer.plus.document'), onSelect: onDocument },
  ]} /> : <IconButton size="sm" label={name} icon={<Paperclip aria-hidden />} disabled={disabled} onClick={onPress} />;
  return <span className="inline-flex items-center gap-0.5">{button}<UploadHelp topic={help} /></span>;
}

interface UploadQueue {
  rows: QueuedUpload[];
  remove: (id: string) => void;
  retry: (id: string) => void;
  retryReason?: string | false;
  progress?: { completed: number; total: number; received: number; size: number };
  paste?: { dirty: (id: string) => void; edit: (id: string, words: string) => void; asText: (id: string, words: string) => void };
}

export function UploadChips({ queue }: { queue: UploadQueue }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const rows = queue.rows.filter((row) => row.replaces === undefined);
  if (rows.length === 0) return null;
  return (
    <ul aria-label={t('uploads.selected')} className="flex max-h-[min(20rem,50dvh)] flex-col gap-1 overflow-y-auto">
      {rows.map((row) => <QueuedAttachment key={row.id} row={row} queue={queue} />)}
      <li role="status" className="text-sm text-text-muted tabular-nums">{queue.progress === undefined ? t('uploads.files', { count: rows.length, number: formatNumber(rows.length, locale) }) : <>{t('uploads.fileProgress', { done: formatNumber(queue.progress.completed, locale), total: formatNumber(queue.progress.total, locale) })} · {t('uploads.bytes', { received: formatBytes(queue.progress.received, locale), total: formatBytes(queue.progress.size, locale) })}</>}</li>
    </ul>
  );
}

function QueuedAttachment({ row, queue }: { row: QueuedUpload; queue: UploadQueue }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const [open, setOpen] = useState(false);
  const editor = useRef<CodeViewHandle>(null);
  const paste = row.kind === 'pasted_text' ? queue.paste : undefined;
  const native = row.work !== undefined && (row.phase === 'writing' || row.phase === 'stopping' && row.work.progress.activity?.stopping === true) ? row.work : undefined;
  const text = useQuery({ queryKey: ['queued-paste', reader, row.id], enabled: paste !== undefined && open, queryFn: () => row.file.text(), retry: false, staleTime: Infinity });
  const asText = () => {
    const words = editor.current?.getText() ?? text.data;
    if (words !== undefined) paste?.asText(row.id, words);
    else void text.refetch().then((reply) => { if (reply.data !== undefined) paste?.asText(row.id, reply.data); });
  };
  return <li className="rounded-md border border-border px-2 py-1 text-sm">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <FileText aria-hidden className="size-4 shrink-0 text-text-muted" />
          {paste === undefined ? <span className={row.kind === 'pasted_text' ? 'max-w-full truncate' : 'min-w-0 flex-1 truncate'}>{row.kind === 'pasted_text' ? t('uploads.pastedText') : row.file.name}</span> : <button type="button" className="focus-inset max-w-full truncate text-left" aria-expanded={open} onClick={() => setOpen(!open)}>{t('uploads.pastedText')}</button>}
          {row.characters !== undefined && <span className="text-text-muted tabular-nums">{t('uploads.characters', { count: row.characters, number: formatNumber(row.characters, locale) })}</span>}
          {row.phase !== 'uploading' && !(row.phase === 'writing' && row.work !== undefined) && <ByteSize bytes={row.file.size} />}
          {native !== undefined ? null : row.phase === 'uploading' ? <span role="status" className="shrink-0 text-text-muted tabular-nums">{t('uploads.bytes', { received: formatBytes(row.received, locale), total: formatBytes(row.file.size, locale) })}</span> : row.phase === 'waiting' ? <span className="text-text-muted">{t('uploads.queued')}</span> : row.phase === 'writing' || row.phase === 'stopping' ? <span role="status" className="text-text-muted">{t(row.phase === 'stopping' ? 'liveWork.stopping' : 'uploads.queued')}</span> : <IconButton size="sm" label={t('uploads.retry', { name: row.kind === 'pasted_text' ? t('uploads.pastedText') : row.file.name })} blocked={typeof queue.retryReason === 'string'} reason={typeof queue.retryReason === 'string' ? queue.retryReason : undefined} icon={<RotateCcw aria-hidden />} onClick={() => queue.retry(row.id)} />}
          <IconButton size="sm" label={t(row.phase === 'failed' ? 'uploads.remove' : 'uploads.cancel', { name: row.kind === 'pasted_text' ? t('uploads.pastedText') : row.file.name })} disabled={row.phase === 'stopping'} icon={<X aria-hidden />} onClick={() => queue.remove(row.id)} />
          {paste !== undefined && <Button size="sm" variant="ghost" disabled={text.isFetching} onClick={asText}>{t('uploads.pasteAsText')}</Button>}
        </div>
        {native !== undefined && <JobActivityLine job={native} />}
        {row.phase === 'failed' && <ErrorAlert error={row.error} />}
        {row.phase === 'stopping' && row.error !== undefined && <ErrorAlert error={row.error} action={<Button size="sm" onClick={() => queue.remove(row.id)}>{t('common.tryAgain')}</Button>} />}
        {open && paste !== undefined && <div className="h-[30dvh] min-h-24 min-w-0">
          {text.isError ? <ErrorAlert error={text.error} action={<Button size="sm" onClick={() => void text.refetch()}>{t('common.tryAgain')}</Button>} /> : text.data === undefined ? <SkeletonRows rows={3} /> : <Suspense fallback={<SkeletonRows rows={3} />}><CodeView ref={editor} text={text.data} version={row.id} label={t('uploads.pastedText')} wrap flushOnUnmount
            onEdit={() => paste.dirty(row.id)} onSettled={(words) => paste.edit(row.id, words)} /></Suspense>}
        </div>}
      </li>;
}

/** Ready files use the same chips in both composers; incompatible images explain the model choice. */
interface AttachmentChipsProps {
  parts: readonly (ContentPart | ChatPart)[];
  onRemove?: (index: number) => void;
  imageIncompatible?: boolean;
  imageReason?: string;
  onChangeModel?: () => void;
  destination?: (part: ContentPart | ChatPart) => string | null;
  readPaste?: (paste: PastedTextPart) => Promise<string>;
  /** Preserves an active editor when editing history allocates its private copy. */
  pastedIdentity?: (paste: PastedTextPart) => string;
  onPasteAsText?: (index: number, text: string) => void;
  onEditPaste?: (index: number, text: string) => void;
  onPasteDirty?: (paste: PastedTextPart, dirty: boolean) => void;
  editText?: (paste: PastedTextPart) => string | undefined;
  editing?: (paste: PastedTextPart) => boolean;
  pasteQueue?: { rows: QueuedUpload[]; retry: (id: string) => void };
  pasteMode?: (paste: PastedTextPart) => boolean | undefined;
  onPasteDelivery?: (paste: PastedTextPart, delivery: 'inline' | 'file') => void;
  pasteTokens?: (paste: PastedTextPart) => number | null;
}

export function AttachmentChips({ parts, onRemove, imageIncompatible = false, imageReason, onChangeModel, pastedIdentity = pasteDisplayKey, ...options }: AttachmentChipsProps) {
  const { t } = useTranslation();
  const hasFiles = parts.some((part) => 'type' in part && (part.type === 'image' || part.type === 'file' || part.type === 'attachment' || part.type === 'pasted_text'));
  if (!hasFiles && !imageIncompatible) return null;
  return <div className="flex flex-col gap-1">
    <ul aria-label={t('uploads.ready')} className="flex flex-wrap gap-1">
      {parts.map((part, index) => !('type' in part) || (part.type !== 'image' && part.type !== 'file' && part.type !== 'attachment' && part.type !== 'pasted_text') ? null : <ReadyAttachment key={part.type === 'pasted_text' ? pastedIdentity(part) : index} part={part} index={index} onRemove={onRemove} {...options} />)}
    </ul>
    {imageIncompatible && (imageReason !== undefined ? <InlineAlert tone="warning">{imageReason}</InlineAlert> : <span role="status" className="inline-flex flex-wrap items-center gap-1 text-sm text-text-muted">{t('fileMedia.imageByPath')}{onChangeModel !== undefined && <> · <button type="button" onClick={onChangeModel} className="underline">{t('uploads.changeModel')}</button></>}<FileGuideHint section="agent" /></span>)}
  </div>;
}

const CodeView = lazy(async () => ({ default: (await import('@/ui/CodeView')).CodeView }));
type ReadyPart = Extract<ContentPart | ChatPart, { type: 'image' | 'file' | 'attachment' | 'pasted_text' }>;

function ReadyAttachment({ part, index, onRemove, destination, readPaste, onPasteAsText, onEditPaste, onPasteDirty, editText, editing, pasteQueue, pasteMode, pasteTokens, onPasteDelivery }: Omit<AttachmentChipsProps, 'parts'> & { part: ReadyPart; index: number }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [open, setOpen] = useState(false);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const shown = useViewShown();
  const editor = useRef<CodeViewHandle>(null);
  const edited = part.type === 'pasted_text' ? editText?.(part) : undefined;
  const draftDirty = part.type === 'pasted_text' && editing?.(part) === true;
  const saving = part.type === 'pasted_text' ? pasteQueue?.rows.find((row) => row.replaces === pasteKey(part)) : undefined;
  const text = useQuery({ queryKey: ['pasted-text', reader, part.blob], enabled: shown && open && part.type === 'pasted_text', retry: false,
    initialData: part.type === 'pasted_text' ? rememberedPaste(part.blob) : undefined,
    queryFn: () => {
      if (part.type !== 'pasted_text') return Promise.resolve('');
      const local = rememberedPaste(part.blob);
      return local !== undefined ? Promise.resolve(local) : readPaste?.(part) ?? Promise.reject(new Error('Pasted text unavailable'));
    },
  });
  const path = destination?.(part);
  const name = part.type === 'pasted_text' ? t('uploads.pastedText') : part.name;
  const characterLabel = part.type === 'pasted_text' ? t('uploads.characters', { count: part.characters, number: formatNumber(part.characters, locale) }) : null;
  const estimatedTokens = part.type === 'pasted_text' ? pasteTokens?.(part) : null;
  const inlinePaste = part.type === 'pasted_text' ? pasteMode?.(part) : undefined;
  return <li className={cx('flex max-w-full min-w-0 flex-col gap-1 rounded-md border border-border px-2 py-1 text-sm', part.type === 'pasted_text' && open && 'w-full')}>
    <div className={cx('flex min-w-0 flex-wrap items-center gap-y-1', part.type === 'pasted_text' ? 'gap-x-1' : 'gap-x-2')}>
      {part.type === 'pasted_text' ? <div className="flex w-full min-w-0 items-center gap-1"><FileText aria-hidden className="size-4 shrink-0 text-text-muted" /><button type="button" className="focus-inset min-w-0 truncate text-left" aria-expanded={open} onClick={() => setOpen(!open)}>{name}</button><UploadHelp topic="pasted-text" />
        <span className="min-w-0 flex-1 truncate text-text-muted tabular-nums" title={characterLabel ?? undefined}>{characterLabel}</span>
        {onRemove !== undefined && <IconButton size="sm" label={t('uploads.remove', { name })} icon={<X aria-hidden />} onClick={() => onRemove(index)} />}</div> : part.blob === null ? part.type === 'file' && <FilePathAttachment path={part.path} name={part.name} size={part.size} /> : <FileAttachment source={blobSource('upload', '', part.blob, part.name, 'size' in part ? part.size : undefined, 'media_type' in part ? part.media_type : undefined)} />}
      {part.type === 'pasted_text' && onPasteDelivery !== undefined && <SegmentedControl size="sm" label={t('uploads.pasteDelivery')} value={part.delivery ?? (inlinePaste === true ? 'inline' : 'file')}
        onValueChange={(value) => onPasteDelivery(part, value)} segments={[{value:'inline',name:t('uploads.inMessage')},{value:'file',name:t('uploads.asFile')}]} />}
      {part.type === 'pasted_text' && onPasteAsText !== undefined && <Button size="sm" variant="ghost" disabled={text.isFetching} onClick={() => {
        const local = editor.current?.getText() ?? edited ?? rememberedPaste(part.blob);
        if (local !== undefined) onPasteAsText(index, local);
        else void text.refetch().then((reply) => { if (reply.data !== undefined) onPasteAsText(index, reply.data); });
      }}>{t('uploads.pasteAsText')}</Button>}
      {onRemove !== undefined && part.type !== 'pasted_text' && <IconButton size="sm" label={t('uploads.remove', { name })} icon={<X aria-hidden />} onClick={() => onRemove(index)} />}
    </div>
    {path !== null && path !== undefined && <span className="min-w-0 truncate text-sm text-text-muted" title={path}>{path}</span>}
    {saving !== undefined && <span className="text-sm text-text-muted" role="status">{t('uploads.savingText')} · {t('uploads.bytes', { received: formatBytes(saving.received, locale), total: formatBytes(saving.file.size, locale) })}</span>}
    {draftDirty && saving === undefined && <span className="text-sm text-text-muted" role="status">{t('uploads.savingText')}</span>}
    {saving?.phase === 'failed' && <ErrorAlert error={saving.error} action={<Button size="sm" onClick={() => pasteQueue?.retry(saving.id)}>{t('common.tryAgain')}</Button>} />}
    {part.type === 'pasted_text' && !draftDirty && saving === undefined && edited === undefined && inlinePaste !== undefined && <span className="text-sm text-text-muted">{t(inlinePaste ? 'uploads.pasteInline' : 'uploads.pasteFile')}{estimatedTokens !== null && estimatedTokens !== undefined && <> · {t('uploads.estimatedTokens', { count: estimatedTokens, number: formatNumber(estimatedTokens, locale) })}</>}</span>}
    {part.type === 'pasted_text' && open && <div className="h-[30dvh] min-h-24 min-w-0">
      {text.isError ? <ErrorAlert error={text.error} action={<Button size="sm" onClick={() => void text.refetch()}>{t('common.tryAgain')}</Button>} /> : text.data === undefined ? <SkeletonRows rows={3} /> : <Suspense fallback={<SkeletonRows rows={3} />}><CodeView ref={editor} text={text.data} version={part.blob} label={t('uploads.pastedText')} wrap readOnly={onEditPaste === undefined} flushOnUnmount={onEditPaste !== undefined}
        overlay={edited === undefined ? undefined : { version: part.blob, text: edited }}
        onEdit={onEditPaste === undefined ? undefined : (dirty) => onPasteDirty?.(part, dirty)}
        onSettled={onEditPaste === undefined ? undefined : (words) => { if (words !== text.data) onEditPaste(index, words); }} /></Suspense>}
    </div>}
    {!open && text.isError && <ErrorAlert error={text.error} />}
  </li>;
}
