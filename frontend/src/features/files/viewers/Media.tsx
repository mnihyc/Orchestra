/** Sound and film that the browser plays.
 *
 * The browser's own player, given the file from the page's memory once it has been read with the
 * person's credentials. A file too large to be held in the page is offered for download instead.
 */
import { useState } from 'react';

import { SkeletonRows } from '@/ui/Skeleton';

import { useFileBytes } from '../content';
import { MEDIA_MOST_BYTES, type ViewerProps } from '../viewers';
import { useObjectUrl } from './Image';
import { FileState, fileFailure } from '../FilePreview';
import { useMediaPreview } from '../mediaPolicy';

export default function MediaViewer({ door, path, head }: ViewerProps) {
  const [failed, setFailed] = useState<string | null>(null);
  const preview = useMediaPreview(door, path, head);
  const tooBig = head.stat.size > (preview.data?.viewer_bytes ?? MEDIA_MOST_BYTES);
  const { bytes, error, loading } = useFileBytes(door, path, tooBig || preview.data === undefined ? undefined : head.brought);
  const type = head.stat.media_type.startsWith('audio/') || head.stat.media_type.startsWith('video/') ? head.stat.media_type : 'video/mp4';
  const url = useObjectUrl(bytes, type);
  if (tooBig) return <FileState state="too_large" />;
  if (preview.isError) return <FileState state={fileFailure(preview.error)} />;
  if (head.brought.state === 'unavailable') return <FileState state={head.brought.reason === 'not_found' ? 'gone' : 'unavailable'} />;
  if (failed !== null && failed === url) return <FileState state="unavailable" />;
  if (error !== null) return <FileState state={fileFailure(error)} />;
  if (loading || url === null) return <SkeletonRows rows={4} />;
  const sound = type.startsWith('audio/');
  return (
    <div className="flex h-full items-center justify-center bg-bg-inset p-4" data-viewer="media">
      {sound ? (
        <audio controls preload="metadata" src={url} onError={() => setFailed(url)} aria-label={head.stat.name} className="w-full max-w-(--container-prose)" />
      ) : (
        // A film may carry no captions of its own; the browser's player offers the ones it has.
        <video controls preload="metadata" src={url} onError={() => setFailed(url)} aria-label={head.stat.name} className="max-h-full max-w-full" />
      )}
    </div>
  );
}
