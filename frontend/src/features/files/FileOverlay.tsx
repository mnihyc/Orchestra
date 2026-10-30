/** Attachments open the workspace's viewer for their kind in the same keyboard-accessible overlay. */
import { useQuery } from '@tanstack/react-query';
import { Suspense, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { SkeletonRows } from '@/ui/Skeleton';
import { FileState, fileFailure } from './FilePreview';
import { FileSourceContext, previewQuery, sourceDownload, sourceHeadQuery, type FileSource } from './source';
import { chosenViewer, factsOf, VIEWERS, type ViewerId, type ViewerProps } from './viewers';
import { lazyPart } from './lazy';
import { FileGuideHint } from './FileGuideHint';

const COMPONENTS = Object.fromEntries(VIEWERS.map((viewer) => [viewer.id, lazyPart<ViewerProps>(viewer.load)] as const));

export function FileOverlay(props: { source: FileSource; onClose: () => void }) {
  return <FileOverlayBody key={`${props.source.url}:${props.source.version ?? ''}`} {...props} />;
}

function FileOverlayBody({ source, onClose }: { source: FileSource; onClose: () => void }) {
  const { t } = useTranslation();
  const head = useQuery(sourceHeadQuery(source));
  const preview = useQuery({ ...previewQuery(source), enabled: false });
  const [choice, setChoice] = useState<ViewerId | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadError, setDownloadError] = useState<unknown>(null);
  const viewer = head.data === undefined ? null : chosenViewer(factsOf(head.data.stat), choice);
  const Loaded = viewer === null ? null : COMPONENTS[viewer.id] ?? null;
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={head.isError && fileFailure(head.error) === 'private' ? t('people.privateReference') : source.name} size="xl"
    footer={<><FileGuideHint section="open" /><Button pending={downloading} blocked={head.isError && fileFailure(head.error) === 'private'} onClick={() => { setDownloading(true); setDownloadError(null); void sourceDownload(source).catch(setDownloadError).finally(() => setDownloading(false)); }}>{t(downloading ? 'fileMedia.downloading' : 'fileMedia.download')}</Button><Button onClick={onClose}>{t('common.close')}</Button></>}>
    {downloadError !== null && <FileState state={fileFailure(downloadError)} />}
    <div className="h-[min(65dvh,48rem)] min-h-48 min-w-0" data-file-overlay="">
      {head.isError ? <FileState state={fileFailure(head.error)} /> : head.data === undefined || Loaded === null ? <SkeletonRows rows={6} /> :
        <FileSourceContext.Provider value={{ ...source, fileId: preview.data?.file_id ?? source.fileId, deliveredPath: preview.data?.path ?? source.deliveredPath }}>
          <Suspense fallback={<SkeletonRows rows={6} />}><Loaded door={source.workspace?.door ?? { environmentId: '', threadId: null }} path={source.workspace?.path ?? source.url}
            head={head.data} line={source.line ?? null} tabId={`attachment:${source.url}`} whyReadOnly={{ kind: 'viewer' }} thread={null} openAs={setChoice} /></Suspense>
        </FileSourceContext.Provider>}
    </div>
  </Dialog>;
}
