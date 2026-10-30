/** Images, fitted, zoomed and moved, with their size; an SVG drawn as an image, with its source beside.
 *
 * The picture is read with the person's credentials and shown from the page's own memory, on a
 * chequered ground, fitted to the room it has; it can be shown at its size, zoomed and moved by the
 * scrollbars. An SVG is drawn as an image, which runs nothing in it; its source is one switch away.
 */
import { Expand, Minus, Plus } from 'lucide-react';
import { useEffect, useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';

import { IconButton } from '@/ui/IconButton';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';

import { useFileBytes, useFileText } from '../content';
import { MEDIA_MOST_BYTES, type ViewerProps } from '../viewers';
import CodeViewer from './Code';
import { CHECKERBOARD, FileState, fileFailure } from '../FilePreview';
import { useMediaPreview } from '../mediaPolicy';

const reducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const motionChanges = (notify: () => void) => {
  const query = window.matchMedia('(prefers-reduced-motion: reduce)');
  query.addEventListener('change', notify);
  return () => query.removeEventListener('change', notify);
};

/** The media type a picture is drawn as: the API's, and for a vector picture the one browsers draw it by. */
function typeOf(name: string, mediaType: string): string {
  if (name.toLowerCase().endsWith('.svg')) return 'image/svg+xml';
  return mediaType.startsWith('image/') ? mediaType : 'image/png';
}

/** The zoom steps: each a quarter more or less of the size before. */
const ZOOM_STEP = 1.25;
const ZOOM_LEAST = 0.1;
const ZOOM_MOST = 16;

/** A picture's address in the page's memory, made when its bytes arrive and given back when it goes. */
export function useObjectUrl(bytes: Uint8Array | null, type: string): string | null {
  const [held, setUrl] = useState<{ bytes: Uint8Array; type: string; url: string } | null>(null);
  useEffect(() => {
    if (bytes === null) return;
    const made = URL.createObjectURL(new Blob([bytes as BlobPart], { type }));
    let live = true;
    // Handed over a moment later: an effect that ran twice in development gives the first address back unused.
    queueMicrotask(() => {
      if (live) setUrl({ bytes, type, url: made });
    });
    return () => {
      live = false;
      URL.revokeObjectURL(made);
    };
  }, [bytes, type]);
  return held?.bytes === bytes && held.type === type ? held.url : null;
}

export default function ImageViewer(props: ViewerProps) {
  const { t } = useTranslation();
  const { door, path, head } = props;
  const isSvg = head.stat.name.toLowerCase().endsWith('.svg');
  const preview = useMediaPreview(door, path, head);
  const tooBig = head.stat.size > (preview.data?.viewer_bytes ?? MEDIA_MOST_BYTES) || preview.data?.state === 'too_large';
  const reduce = useSyncExternalStore(motionChanges, reducedMotion, () => false);
  const [animate, setAnimate] = useState(false);
  const [withSource, setWithSource] = useState(false);
  const staticImage = reduce && !animate && /\.(gif|webp|avif|svg)$/i.test(head.stat.name);
  const safe = preview.data?.state === 'ready' && preview.data.image !== null;
  const { bytes, error, loading } = useFileBytes(door, path, tooBig || staticImage || !safe ? undefined : head.brought);
  const source = useFileText(door, path, isSvg && withSource ? head.brought : undefined);
  const url = useObjectUrl(bytes, typeOf(head.stat.name, head.stat.media_type));
  const [zoom, setZoom] = useState<number | 'fit'>('fit');
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const displayed = staticImage ? preview.data?.image ?? null : url;

  if (tooBig) return <FileState state="too_large" />;
  if (head.brought.state === 'unavailable') return <FileState state={head.brought.reason === 'not_found' ? 'gone' : 'unavailable'} />;
  if (preview.isError) return <FileState state={fileFailure(preview.error)} />;
  if (preview.data !== undefined && !safe) return <FileState state="unavailable" />;
  if (error !== null) return <FileState state={fileFailure(error)} />;
  if (loading || displayed === null) return <SkeletonRows rows={6} />;
  if (failed === displayed) return <FileState state="unavailable" />;

  const step = (factor: number) => setZoom((current) => Math.min(ZOOM_MOST, Math.max(ZOOM_LEAST, (current === 'fit' ? 1 : current) * factor)));
  return (
    <div className="@container flex h-full min-h-0 flex-col" data-viewer="image">
      <div className="flex flex-wrap items-center gap-1 border-b border-border px-2 py-1">
        <Button size="sm" variant={zoom === 'fit' ? 'secondary' : 'ghost'} icon={<Expand aria-hidden />} onClick={() => setZoom('fit')}>
          {t('files.image.fit')}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setZoom(1)}>
          {t('files.image.actual')}
        </Button>
        <IconButton size="sm" label={t('files.image.zoomOut')} icon={<Minus aria-hidden />} onClick={() => step(1 / ZOOM_STEP)} />
        <IconButton size="sm" label={t('files.image.zoomIn')} icon={<Plus aria-hidden />} onClick={() => step(ZOOM_STEP)} />
        <span className="flex-1" />
        {staticImage && <Button size="sm" variant="ghost" onClick={() => setAnimate(true)}>{t('fileMedia.playAnimation')}</Button>}
        {size !== null && <span className="text-xs text-text-muted tabular-nums">{t('files.image.pixels', { width: size.width, height: size.height })}</span>}
        {isSvg && (
          <Button size="sm" variant={withSource ? 'secondary' : 'ghost'} onClick={() => setWithSource((on) => !on)}>
            {t('files.image.source')}
          </Button>
        )}
      </div>
      <div className={withSource ? 'grid min-h-0 flex-1 grid-cols-1 grid-rows-2 gap-px bg-border @3xl:grid-cols-2 @3xl:grid-rows-1' : 'min-h-0 flex-1'}>
        <div
          className="flex h-full min-h-0 min-w-0 overflow-auto bg-surface"
          style={CHECKERBOARD}
        >
          <img
            src={displayed}
            alt={head.stat.name}
            onError={() => setFailed(displayed)}
            onLoad={(event) => setSize({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })}
            className={zoom === 'fit' ? 'm-auto max-h-full max-w-full object-contain' : 'm-auto max-w-none shrink-0'}
            style={zoom === 'fit' || size === null ? undefined : { width: size.width * zoom, height: size.height * zoom }}
          />
        </div>
        {withSource && source.text !== null && (
          <div className="min-h-0 min-w-0 bg-surface">
            <CodeViewer {...props} whyReadOnly={{ kind: 'viewer' }} />
          </div>
        )}
      </div>
    </div>
  );
}
