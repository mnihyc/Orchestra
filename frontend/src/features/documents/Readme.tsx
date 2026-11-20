/** The README of a place at the top of its view: a bounded preview with a path to the whole document.
 *
 * It is read here and written on its own page. A thread shows it folded to one line once the reader has
 * read the document as it stands, and opens it again with a press. Somebody who may not read the
 * document is shown no README at all: the place's view says nothing of it to them.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { BookOpen, ChevronDown, ChevronRight } from 'lucide-react';
import { type ComponentProps, Suspense, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchDocumentSync } from '@/api/documents';
import { updateNode } from '@/api/nodes';
import { settingsQuery } from '@/api/settings';
import { useViewShown } from '@/features/workbench/viewOpening';
import { compareBlocks } from '@/features/documents/blocks';
import type { BlockEditor as BlockEditorView } from '@/features/documents/Editor';
import { lazyPart } from '@/features/files/lazy';
import type { Document, DocumentBlock, DocumentRef, Node } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { cx } from '@/ui/cx';
import { useErrorText } from '@/ui/errorText';
import { quietButtonClasses } from '@/ui/link';
import type { MenuEntry } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { ErrorAlert } from '@/ui/ErrorState';
import { Button } from '@/ui/Button';
import { toast } from '@/ui/Toast';

/**
 * The chosen top-level blocks before the first page break, with all their children; and whether more remains.
 */
export function readmeCut(blocks: readonly DocumentBlock[], maxBlocks: number): { shown: DocumentBlock[]; more: boolean } {
  const top = blocks.filter((block) => block.parent_id === null && !block.removed).sort(compareBlocks);
  const stop = top.findIndex((block) => block.type === 'page_break');
  const end = stop >= 0 ? Math.min(stop, maxBlocks) : maxBlocks;
  const kept = top.slice(0, end);
  const more = top.slice(kept.length).some((block) => block.type !== 'page_break');
  const keep = new Set(kept.map((block) => block.id));
  // What lies inside a kept block goes with it, however deep.
  for (let grew = true; grew; ) {
    grew = false;
    for (const block of blocks) {
      if (!block.removed && block.parent_id !== null && keep.has(block.parent_id) && !keep.has(block.id)) {
        keep.add(block.id);
        grew = true;
      }
    }
  }
  return { shown: blocks.filter((block) => keep.has(block.id)), more };
}

/**
 * The document menu makes it its place's README, or stops using it there. Missing place metadata
 * or Manage keeps the same entry disabled with its reason.
 */
export function useReadmeEntry(doc: Pick<Document, 'id'>, place: Node | undefined): MenuEntry | null {
  const { t } = useTranslation();
  const describe = useErrorText();
  const change = useMutation({
    mutationFn: ({ node, to }: { node: Node; to: string | null }) => updateNode(node.id, { readme_document_id: to, expected_revision: node.revision }),
    onSuccess: (node, { to }) => {
      toast({ tone: 'success', title: to === null ? t('documents.readme.stopped', { place: node.name }) : t('documents.readme.used', { place: node.name }) });
    },
    onError: (error) => toast({ tone: 'danger', title: t('documents.readme.failed'), description: describe(error).message }),
  });
  const reason = place === undefined ? t('implicitAvailability.placeUnavailable')
    : !holds(place.access, `${place.kind}.manage`) ? t('implicitAvailability.placeManage') : false;
  const inUse = place?.readme?.id === doc.id;
  return {
    id: 'readme',
    label: inUse ? t('documents.readme.stop') : t('documents.readme.use'),
    disabled: reason,
    onSelect: () => { if (place !== undefined && reason === false) change.mutate({ node: place, to: inUse ? null : doc.id }); },
  };
}

/** Whether the reader has had the document open as it stands now. */
function wasRead(readme: DocumentRef): boolean {
  return readme.seen_seq !== null && readme.seen_seq >= readme.seq;
}

/** The document editor draws the README; it is fetched when a README is first drawn, never with the sidebar that offers its menu entry. */
const BlockEditor = lazyPart<ComponentProps<typeof BlockEditorView>>(() => import('@/features/documents/Editor').then((module) => ({ default: module.BlockEditor })));

export function PlaceReadme({ readme, nodeId, foldable = false }: { readme: DocumentRef; nodeId: string; foldable?: boolean }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const [open, setOpen] = useState(() => !foldable || !wasRead(readme));
  const settings = useQuery({ ...settingsQuery(nodeId), enabled: open && shown });
  const sync = useQuery({
    queryKey: ['readme', readme.id, readme.seq],
    queryFn: ({ signal }) => fetchDocumentSync(readme.id, null, signal),
    enabled: open && shown,
  });
  const allowance = settings.data?.find((one) => one.key === 'documents.readme_blocks')?.value;
  const maxBlocks = typeof allowance === 'number' && allowance >= 1 ? allowance : null;
  const cut = sync.data === undefined || maxBlocks === null ? null : readmeCut(sync.data.blocks, maxBlocks);
  const failure = sync.error ?? settings.error ?? (settings.isSuccess && maxBlocks === null ? new Error(t('documents.readme.unavailable')) : null);
  const title = (
    <span className="flex min-w-0 items-center gap-2">
      <BookOpen aria-hidden className="size-4 shrink-0 text-text-muted" />
      <span className="truncate text-sm font-medium text-text">{readme.title}</span>
    </span>
  );
  return (
    <section aria-label={t('documents.readme.label', { title: readme.title })} className="rounded-lg border border-border bg-surface" data-readme={readme.id}>
      {foldable ? (
        <button
          type="button"
          aria-expanded={open}
          className={cx('flex w-full items-center justify-between gap-2 px-4 py-2 text-left', quietButtonClasses)}
          onClick={() => setOpen((now) => !now)}
        >
          {title}
          {open ? <ChevronDown aria-hidden className="size-4 shrink-0" /> : <ChevronRight aria-hidden className="size-4 shrink-0" />}
        </button>
      ) : (
        <div className="flex items-center justify-between gap-2 px-4 pt-3">{title}</div>
      )}
      {open && (
        <div className="px-4 pb-3">
          {failure !== null ? <ErrorAlert error={failure} action={<Button size="sm" onClick={() => { void sync.refetch(); void settings.refetch(); }}>{t('common.tryAgain')}</Button>} /> : sync.data === undefined || cut === null ? (
            <SkeletonRows read={sync} rows={3} />
          ) : (
            <Suspense fallback={<SkeletonRows read={sync} rows={3} />}><BlockEditor document={sync.data.document} editable={false} inspection fixed={cut.shown} /></Suspense>
          )}
          <div className="flex flex-wrap items-center gap-2 text-sm">
            {cut?.more === true && <span className="text-text-muted">{t('documents.readme.preview')}</span>}
            <Link to="/doc/$documentId" params={{ documentId: readme.id }} className="text-sm font-medium text-accent hover:underline focus-visible:outline-2 focus-visible:outline-focus">
              {t('documents.readme.open')}
            </Link>
          </div>
        </div>
      )}
    </section>
  );
}
