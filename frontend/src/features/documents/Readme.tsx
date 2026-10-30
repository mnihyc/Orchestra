/** The README of a place at the top of its view: the document it named, up to that document's first page break or its first twelve blocks where it has none, with Read more, which opens the document.
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
import { toast } from '@/ui/Toast';

/** How many top-level blocks a README shows where its document has no page break: the top of a screen. */
export const README_BLOCKS_MAX = 12;

/**
 * The blocks a README shows: the top-level ones before the document's first page break, or its first
 * [`README_BLOCKS_MAX`] where it has none, each with everything inside it; and whether the document holds more.
 */
export function readmeCut(blocks: readonly DocumentBlock[]): { shown: DocumentBlock[]; more: boolean } {
  const top = blocks.filter((block) => block.parent_id === null && !block.removed).sort(compareBlocks);
  const stop = top.findIndex((block) => block.type === 'page_break');
  const kept = stop >= 0 ? top.slice(0, stop) : top.slice(0, README_BLOCKS_MAX);
  const more = stop >= 0 ? top.length > stop + 1 : top.length > kept.length;
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
 * The entry of a document's menu that makes it its place's README, or on the one in use stops that; none for
 * somebody who may not Edit the place.
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
  if (place === undefined || !holds(place.access, `${place.kind}.manage`)) return null;
  const inUse = place.readme?.id === doc.id;
  return {
    id: 'readme',
    label: inUse ? t('documents.readme.stop') : t('documents.readme.use'),
    onSelect: () => change.mutate({ node: place, to: inUse ? null : doc.id }),
  };
}

/** Whether the reader has had the document open as it stands now. */
function wasRead(readme: DocumentRef): boolean {
  return readme.seen_seq !== null && readme.seen_seq >= readme.seq;
}

/** The document editor draws the README; it is fetched when a README is first drawn, never with the sidebar that offers its menu entry. */
const BlockEditor = lazyPart<ComponentProps<typeof BlockEditorView>>(() => import('@/features/documents/Editor').then((module) => ({ default: module.BlockEditor })));

export function PlaceReadme({ readme, foldable = false }: { readme: DocumentRef; foldable?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(() => !foldable || !wasRead(readme));
  const sync = useQuery({
    queryKey: ['readme', readme.id, readme.seq],
    queryFn: ({ signal }) => fetchDocumentSync(readme.id, null, signal),
    enabled: open,
  });
  const cut = sync.data === undefined ? null : readmeCut(sync.data.blocks);
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
          {sync.data === undefined || cut === null ? (
            <SkeletonRows rows={3} />
          ) : (
            <Suspense fallback={<SkeletonRows rows={3} />}><BlockEditor document={sync.data.document} editable={false} inspection fixed={cut.shown} /></Suspense>
          )}
          {cut?.more === true && (
            <Link to="/doc/$documentId" params={{ documentId: readme.id }} className="text-sm font-medium text-accent hover:underline focus-visible:outline-2 focus-visible:outline-focus">
              {t('documents.readme.more')}
            </Link>
          )}
        </div>
      )}
    </section>
  );
}
