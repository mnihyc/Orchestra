/** A destination folder can also be found by walking its place's paged document tree. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronLeft, Folder } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { documentDirectoryQuery } from '@/api/documentTree';
import type { ImportDestinationName } from '@/features/people/ImportDestination';
import type { DocumentDirectory } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Tooltip } from '@/ui/Tooltip';

interface Step { at: DocumentDirectory; name: string | null; cursors: (string | null)[] }

/** Search can be limited globally; browsing retains the full visible directory, a page at a time. */
export function ImportFolderBrowser({ place, onChoose }: { place: string; onChoose: (folder: ImportDestinationName | null) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <Popover open={open} onOpenChange={setOpen} label={t('importWizard.destination.folderBrowse')}
    trigger={<Button size="sm" icon={<Folder aria-hidden />}>{t('importWizard.destination.folderBrowse')}</Button>}>
    {open && <FolderContents place={place} onChoose={(folder) => { onChoose(folder); setOpen(false); }} />}
  </Popover>;
}

function FolderContents({ place, onChoose }: { place: string; onChoose: (folder: ImportDestinationName | null) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const root: Step = { at: { kind: 'node_docs', id: place }, name: null, cursors: [null] };
  const [trail, setTrail] = useState<Step[]>([root]);
  const current = trail.at(-1) ?? root;
  const pathRef = useRef<HTMLSpanElement>(null);
  useEffect(() => { pathRef.current?.focus({ preventScroll: true }); }, [current.at.kind, current.at.id]);
  const cursor = current.cursors.at(-1) ?? null;
  const page = useQuery({ ...documentDirectoryQuery({ node: place, at: current.at, cursor }, cache), gcTime: 0 });
  const directory = page.data?.directory;
  const valid = directory?.node.id === place && directory.at.kind === current.at.kind && directory.at.id === current.at.id
    && (current.at.kind === 'node_docs' || directory.folder?.entry.item.id === current.at.id);
  const allowed = valid && directory.can_create && !page.isFetching && !page.isError;
  const children = (page.data?.items ?? []).filter((row) => row.entry.item.kind === 'container' && row.entry.deleted_at === null);
  const nextCursor = page.data?.next_cursor ?? null;
  const path = trail.map((step) => step.name ?? t('documents.list.title')).join(' / ');
  const cursors = (next: (string | null)[]) => setTrail((before) => before.map((step, index) => index === before.length - 1 ? { ...step, cursors: next } : step));
  return <div className="flex w-[min(20rem,calc(100vw-3.5rem))] min-w-0 flex-col gap-2">
      <div className="flex min-w-0 items-center gap-2">
        {trail.length > 1 && <Button size="sm" variant="ghost" icon={<ChevronLeft aria-hidden />} onClick={() => setTrail((before) => before.slice(0, -1))}>{t('importWizard.destination.folderParent')}</Button>}
        <span ref={pathRef} tabIndex={-1} className="focus-inset min-w-0 truncate text-sm text-text-muted" title={path}>{path}</span>
      </div>
      {page.isPending ? <SkeletonRows rows={3} /> : page.isError ? <ErrorAlert error={page.error} action={<Button size="sm" onClick={() => void page.refetch()}>{t('common.tryAgain')}</Button>} /> : <>
        <Tooltip content={allowed ? undefined : t(page.isFetching ? 'importWizard.destination.checking' : 'importWizard.destination.folderDenied')}><Button size="sm" blocked={!allowed} onClick={() => {
          onChoose(current.at.kind === 'node_docs' ? null : { id: current.at.id, label: directory?.folder?.entry.title ?? current.name ?? t('ui.nameUnknown') });
        }}>{t('importWizard.destination.folderUse')}</Button></Tooltip>
        {children.length === 0 ? <StateLine compact message={t('importWizard.destination.folderPageEmpty')} /> : <ul className="divide-y divide-border">
          {children.map((row) => <li key={row.entry.item.id}><Button size="sm" variant="ghost" block disabled={page.isFetching} className="justify-start" icon={<Folder aria-hidden />} onClick={() => setTrail((before) => [...before, { at: { kind: 'container', id: row.entry.item.id }, name: row.entry.basename, cursors: [null] }])}>
            <span className="min-w-0 truncate" title={row.entry.basename}>{row.entry.basename}</span>
          </Button></li>)}
        </ul>}
        {(current.cursors.length > 1 || nextCursor !== null) && <div className="flex flex-wrap gap-2">
          <Button size="sm" disabled={current.cursors.length <= 1 || page.isFetching} onClick={() => cursors(current.cursors.slice(0, -1))}>{t('importWizard.destination.folderPrevious')}</Button>
          <Button size="sm" disabled={nextCursor === null || page.isFetching} onClick={() => { if (nextCursor !== null) cursors([...current.cursors, nextCursor]); }}>{t('importWizard.destination.folderNext')}</Button>
        </div>}
      </>}
  </div>;
}
