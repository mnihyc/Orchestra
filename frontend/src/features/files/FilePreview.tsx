/** One lazy bounded preview and the same unavailable states in every file surface. */
import { useQuery } from '@tanstack/react-query';
import { File, FileArchive, FileCode, FileImage, FileMusic, FileSpreadsheet, FileText, FileVideo } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { Skeleton } from '@/ui/Skeleton';
import { cx } from '@/ui/cx';
import { defaultViewer } from './viewers';
import { previewQuery, sourceFacts, type FileSource, type FilePreview as PreviewData } from './source';

export const CHECKERBOARD = { backgroundImage: 'conic-gradient(var(--bg-muted) 25%, transparent 0 50%, var(--bg-muted) 0 75%, transparent 0)', backgroundSize: '1rem 1rem' };

export function FileGlyph({ source, preview, className }: { source: FileSource; preview?: Pick<PreviewData, 'media_type' | 'size' | 'is_text'> | undefined; className?: string }) {
  const facts = sourceFacts(source, preview);
  const kind = defaultViewer(facts).id;
  const Icon = kind === 'image' ? FileImage : kind === 'archive' || /\.(7z|rar|jar)$/i.test(source.name) ? FileArchive : /\.(xls|xlsx|ods)$/i.test(source.name) ? FileSpreadsheet : kind === 'pdf' || kind === 'markdown' || /\.(doc|docx|ppt|pptx|odt|odp)$/i.test(source.name) ? FileText : kind === 'code' || kind === 'difference' ? FileCode : kind === 'media' ? facts.mediaType.startsWith('audio/') || /\.(mp3|wav|ogg|oga|flac|aac|m4a|opus)$/i.test(source.name) ? FileMusic : FileVideo : File;
  return <Icon aria-hidden className={cx('size-4 shrink-0 text-text-muted', className)} />;
}

export function fileFailure(error: unknown): 'private' | 'gone' | 'unavailable' | 'too_large' {
  if (!(error instanceof ApiError)) return 'unavailable';
  if (error.status === 401 || error.status === 403) return 'private';
  if (error.status === 404 || error.status === 410) return 'gone';
  if (error.status === 413) return 'too_large';
  return 'unavailable';
}

export function FileState({ state }: { state: ReturnType<typeof fileFailure> }) {
  const { t } = useTranslation();
  return <span className="text-sm text-text-muted" role="status">{t(state === 'private' ? 'people.privateReference' : state === 'gone' ? 'people.reference.fileMissing' : state === 'too_large' ? 'fileMedia.tooLarge' : 'fileMedia.unavailable')}</span>;
}

/** An offscreen preview makes no request. Once shown it stays in the query cache for back-scrolling. */
export function useFileVisible(enabled = true) {
  const ref = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!enabled) return;
    const node = ref.current;
    if (node === null) return;
    if (typeof IntersectionObserver === 'undefined') {
      let active = true;
      queueMicrotask(() => { if (active) setVisible(true); });
      return () => { active = false; };
    }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) { setVisible(true); observer.disconnect(); }
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [enabled]);
  return { ref, visible };
}

/** All kinds share this slot: an image when cheap, otherwise its glyph and a truthful state. */
export function FilePreview({ source, details = false, enabled = true }: { source: FileSource; details?: boolean; enabled?: boolean }) {
  const { ref, visible } = useFileVisible();
  const hint = sourceFacts(source);
  const hintedViewer = defaultViewer(hint).id;
  const hintedVisual = hintedViewer === 'image' || hintedViewer === 'pdf' || hintedViewer === 'media' && !hint.mediaType.startsWith('audio/');
  const query = useQuery({ ...previewQuery(source), enabled: enabled && visible && (details || hintedVisual) });
  const [failedImage, setFailedImage] = useState<string | null>(null);
  const preview = query.error === null && !query.isFetching ? query.data : undefined;
  // Keep the known preview frame during a refetch without showing stale preview bytes.
  const facts = sourceFacts(source, query.data);
  const viewer = defaultViewer(facts).id;
  const visual = viewer === 'image' || viewer === 'pdf' || viewer === 'media' && !facts.mediaType.startsWith('audio/');
  const text = preview?.text;
  const state = query.isError ? fileFailure(query.error) : preview?.state === 'too_large' ? 'too_large' : 'unavailable';
  return <span ref={ref} className={cx('flex max-w-full flex-col gap-1 overflow-hidden rounded-md', visual && 'h-28 w-48 items-center justify-center border border-border bg-bg-subtle')} data-file-preview="">
    {query.isError && state === 'private' ? <FileState state="private" /> : (query.isPending || query.isFetching) && (visual || details) ? <Skeleton className={visual ? 'h-full w-full' : 'h-16 w-full'} /> : visual && preview?.image !== null && preview?.image !== undefined && failedImage !== preview.image ?
      <img src={preview.image} alt={source.name} loading="lazy" decoding="async" style={CHECKERBOARD} className="h-full w-full object-contain" onError={() => setFailedImage(preview.image)} /> : <>
        <FileGlyph source={source} preview={query.data} className={visual ? 'size-6' : undefined} />
        {(visual || details) && (text === null || text === undefined) && (visual || preview?.state !== 'ready') && <FileState state={state} />}
        {details && typeof text === 'string' && <span className="max-h-28 overflow-hidden whitespace-pre-wrap font-mono text-xs wrap-anywhere">{text}</span>}
      </>}
  </span>;
}
