/** One file chip and thumbnail family, used in messages, tool results, documents and file lists. */
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { ByteSize } from '@/ui/ByteSize';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { cx } from '@/ui/cx';
import { defaultViewer } from './viewers';
import { FileGlyph, FilePreview, FileState, fileFailure } from './FilePreview';
import { FileOverlay } from './FileOverlay';
import { previewQuery, sourceFacts, sourceHasReference, sourceReference, type FileSource } from './source';
import { ORCH_REFERENCE } from '@/lib/drop';
import { useChipPresentation, type ChipPresentation } from '@/features/people/chipSizes';
import type { TypedReference } from '@/features/people/references';
import type { MenuEntry } from '@/ui/Menu';

export function FileChip({ source, presentation, reference, onOpen, bare = false, label, ariaLabel, menuEntries }: { source: FileSource; presentation?: ChipPresentation; reference?: TypedReference; onOpen?: (event: React.MouseEvent<HTMLElement>) => void; bare?: boolean; label?: React.ReactNode; ariaLabel?: string; menuEntries?: () => MenuEntry[] }) {
  const preview = useQuery({ ...previewQuery(source), enabled: false });
  const canDrag = source.href !== undefined && sourceHasReference(source);
  const drag = { draggable: canDrag, onDragStart: (event: React.DragEvent<HTMLElement>) => {
    if (!canDrag) return;
    event.dataTransfer.setData(ORCH_REFERENCE, sourceReference(source));
    event.dataTransfer.effectAllowed = 'copy';
  } };
  const className = cx('focus-inset inline-flex max-w-full items-center gap-1', bare ? 'rounded-sm text-text' : 'rounded-md border border-border bg-bg-subtle px-2 py-1 text-sm text-text hover:bg-bg-muted');
  if (preview.isError && fileFailure(preview.error) === 'private') return <FileState state="private" />;
  return <ReferenceCard {...(reference === undefined ? { source } : { reference })} label={source.name} presentation={presentation} menuEntries={menuEntries}>
    {(keep, size) => {
      const bytes = preview.data?.size ?? source.size;
      const content = <><FileGlyph source={source} preview={preview.data} /><span className="min-w-0 truncate">{label ?? source.name}</span>{size !== 'small' && <span className="shrink-0 text-sm text-text-muted">{bytes === undefined ? '–' : <ByteSize bytes={bytes} />}</span>}</>;
      return source.href === undefined ? <button {...drag} type="button" aria-label={ariaLabel ?? (typeof label === 'string' ? label : source.name)} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') event.stopPropagation(); }} onClick={(event) => { event.stopPropagation(); if (onOpen === undefined) keep(); else onOpen(event); }} title={source.name} className={className}>{content}</button> :
        <a {...drag} href={source.href} aria-label={ariaLabel ?? source.name} title={source.name} className={className} onClick={(event) => { if (event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) { event.preventDefault(); event.stopPropagation(); if (onOpen ===undefined) keep(); else onOpen(event); } }}>{content}</a>;
    }}
  </ReferenceCard>;
}

/** ImageThumb remains the family for visual attachments: bounded, lazy, and opening the normal viewer. */
export function FileAttachment({ source, presentation, thumbnail = true }: { source: FileSource; presentation?: ChipPresentation; thumbnail?: boolean }) {
  const { t } = useTranslation();
  const [viewing, setViewing] = useState(false);
  const preview = useQuery({ ...previewQuery(source), enabled: false });
  const { size } = useChipPresentation(presentation);
  const facts = sourceFacts(source, preview.data);
  const viewer = defaultViewer(facts).id;
  const visual = viewer === 'image' || viewer === 'pdf' || viewer === 'media' && !facts.mediaType.startsWith('audio/');
  if (preview.isError && fileFailure(preview.error) === 'private') return <FileState state="private" />;
  return <span className="inline-flex max-w-full flex-col items-start gap-1 align-top" data-file-attachment="">
    {thumbnail && visual && size !== 'large' && presentation?.place !== 'compact' && <button type="button" aria-label={t('fileMedia.openFile', { name: source.name })} onClick={() => setViewing(true)} className="focus-inset block max-w-full rounded-md"><FilePreview source={source} /></button>}
    <FileChip source={source} presentation={presentation} />
    {viewing && <FileOverlay source={source} onClose={() => setViewing(false)} />}
  </span>;
}
