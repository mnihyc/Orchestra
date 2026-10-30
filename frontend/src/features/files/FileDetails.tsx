/** A file card's preview and actions are shared by attachments, references and cards in document flow. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { copyText } from '@/ui/clipboard';
import { AuthorChip } from '@/ui/AuthorChip';
import { RelativeTime } from '@/ui/RelativeTime';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { previewQuery, sourceDownload, sourceHasReference, sourceReference, type FileSource } from './source';
import { FileGlyph, FilePreview, FileState, fileFailure, useFileVisible } from './FilePreview';
import { defaultViewer } from './viewers';
import { sourceFacts } from './source';
import { FileGuideHint } from './FileGuideHint';

export function FileDetails({ source, onOpen, onDone, title = true, showPreview = true, showMetadata = true }: { source: FileSource; onOpen: () => void; onDone?: (() => void) | undefined; title?: boolean; showPreview?: boolean; showMetadata?: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { locale } = format;
  const { ref, visible } = useFileVisible();
  const preview = useQuery({ ...previewQuery(source), enabled: visible && showPreview });
  const [downloadError, setDownloadError] = useState<unknown>(null);
  const [downloading, setDownloading] = useState(false);
  if (preview.isError && fileFailure(preview.error) === 'private') return <FileState state="private" />;
  const data = preview.error === null ? preview.data : undefined;
  const gone = preview.isError && fileFailure(preview.error) === 'gone';
  const kind = defaultViewer(sourceFacts(source, data)).id;
  const referenceSource = { ...source, fileId: data?.file_id ?? source.fileId, deliveredPath: data?.path ?? source.deliveredPath };
  const hasReference = sourceHasReference(referenceSource);
  return <span ref={ref} className="flex w-[min(20rem,calc(100vw-4rem))] max-w-full flex-col gap-2" data-file-card="">
    {title && <span className="flex min-w-0 items-center gap-2"><FileGlyph source={source} preview={data} /><span className="min-w-0 font-medium wrap-anywhere">{source.name}</span></span>}
    {showPreview && <button type="button" aria-label={t('fileMedia.openFile', { name: source.name })} disabled={gone} className="focus-inset max-w-full rounded-md text-left" onClick={() => { onOpen(); onDone?.(); }}><FilePreview source={source} details /></button>}
    {source.description && <span className="line-clamp-1 text-sm text-text-muted">{source.description.split('\n')[0]}</span>}
    {showMetadata && <span className="flex flex-wrap items-center gap-1 text-sm text-text-muted">
      <span>{t('fileMedia.owner')}</span>{source.owner === undefined ? <span>–</span> : <AuthorChip author={source.owner} />}
      <span aria-hidden>·</span><span>{t('fileMedia.changed')}</span>{source.updatedAt === undefined ? <span>–</span> : <RelativeTime at={source.updatedAt} />}
    </span>}
    {showMetadata && <span className="flex flex-wrap gap-x-2 text-sm text-text-muted">
      {(data?.size ?? source.size) !== undefined && <ByteSize bytes={data?.size ?? source.size ?? 0} />}
      {data?.media_type && <span>{data.media_type}</span>}
      {kind === 'media' && <span>{t('fileMedia.duration', { value: data?.duration_seconds === null || data?.duration_seconds === undefined ? '–' : formatSpan(data.duration_seconds * 1000, format) })}</span>}
      {kind === 'archive' && <span>{t('fileMedia.entries', { number: data?.entry_count === null || data?.entry_count === undefined ? '–' : formatNumber(data.entry_count, locale) })}</span>}
    </span>}
    {gone && !showPreview && <FileState state="gone" />}
    {downloadError !== null && <FileState state={fileFailure(downloadError)} />}
    <span className="flex flex-wrap gap-2">
      <Button size="sm" variant="primary" disabled={gone} onClick={() => { onOpen(); onDone?.(); }}>{t('fileMedia.open')}</Button>
      <Button size="sm" disabled={gone} pending={downloading} onClick={() => { setDownloadError(null); setDownloading(true); void sourceDownload(source).catch(setDownloadError).finally(() => setDownloading(false)); }}>{t(downloading ? 'fileMedia.downloading' : 'fileMedia.download')}</Button>
      <Button size="sm" variant="ghost" blocked={!hasReference} title={hasReference ? undefined : t('fileMedia.noReference')} aria-description={hasReference ? undefined : t('fileMedia.noReference')} onClick={() => { if (hasReference) void copyText(sourceReference(referenceSource), t('people.reference.copied')); }}>{t('people.reference.copy')}</Button>
      <FileGuideHint section="previews" />
    </span>
  </span>;
}
