/** The History drawer: each stretch of writing with who wrote and how much, people and agents alike, what it changed, the document as it stood then, and putting a version back.
 *
 * An entry is a stretch of writing, merged: everything that was changed between two versions, whoever
 * wrote and however many times they came back to it. Whoever wrote is an author, a person and an agent
 * alike. Choosing an entry shows what it changed, block by block, with who changed each. "Put this version
 * back" makes it the document again after keeping the present one as a version, so that nothing is lost.
 */
import { useListChoice } from '@/lib/listState';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Columns2, Rows2 } from 'lucide-react';
import { useMemo, useState } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';

import { client } from '@/api/client';
import { changesQuery, restoreVersion, versionsQuery, type Document, type DocumentChanges, type DocumentVersion } from '@/api/documents';
import { plainText, type Content } from '@/features/documents/blocks';
import { diffWords } from '@/features/documents/diff';
import { PutBack } from '@/features/people/PutBack';
import { authorName } from '@/features/documents/Together';
import { AuthorChip } from '@/ui/AuthorChip';
import type { Author } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { documentWriter } from '@/realtime/stores/document';
import { refreshDocument } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { Drawer } from '@/ui/Drawer';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Pagination } from '@/ui/Pagination';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';

/** Whether the document changed since the reader last had it open: the clock gets a dot. */
export function useHistoryDot(doc: Document): boolean {
  const me = client.session.getState().userId;
  const by = doc.updated_by;
  return doc.seen_seq !== null && doc.seq > doc.seen_seq && !(by !== null && 'id' in by && by.id === me);
}

/** What an entry's block came to: it was added, removed, or changed. */
function changeKind(one: { before: unknown; after: unknown }): 'added' | 'removed' | 'changed' {
  return one.before === null ? 'added' : one.after === null ? 'removed' : 'changed';
}

/** What an entry is told apart by in the list: its number, or "open" for the stretch that is not a version yet. */
function entryKey(entry: DocumentVersion): number | 'open' {
  return entry.version ?? 'open';
}

const authorKey = (author: Author): string => ('id' in author ? author.id : `thread:${author.thread.id}`);

/** "6 blocks changed, 2 added" for an entry. */
type FileChange = NonNullable<DocumentVersion['changed']['files']>[number];
const FILE_COUNTS = { added: 'documents.history.filesAdded', replaced: 'documents.history.filesReplaced', removed: 'documents.history.filesRemoved' } as const;

function summary(say: (key: 'changed' | 'added' | 'removed', count: number) => string, changed: DocumentVersion['changed'], sayFile: (kind: FileChange['kind'], count: number) => string): string {
  const parts: string[] = [];
  const edited = changed.updated + changed.moved;
  if (edited > 0) parts.push(say('changed', edited));
  if (changed.inserted > 0) parts.push(say('added', changed.inserted));
  if (changed.removed > 0) parts.push(say('removed', changed.removed));
  for (const kind of ['added', 'replaced', 'removed'] as const) {
    const count = (changed.files ?? []).filter((file) => file.kind === kind).length;
    if (count > 0) parts.push(sayFile(kind, count));
  }
  return parts.join(', ');
}

function FileChanges({ files }: { files: readonly FileChange[] }) {
  const { t } = useTranslation();
  if (files.length === 0) return null;
  return <ul aria-label={t('documents.history.fileDifference')} className="flex flex-col gap-2">
    {files.map((file, index) => <li key={`${file.kind}:${file.name}:${index}`} className="wrap-anywhere text-sm text-text">
      {t(`documents.history.file_${file.kind}`, { name: file.name })}
    </li>)}
  </ul>;
}

// ---------------------------------------------------------------- what an entry changed

function Words({ before, after }: { before: string; after: string }) {
  return (
    <span className="whitespace-pre-wrap">
      {diffWords(before, after).map((piece, index) =>
        piece.kind === 'same' ? (
          <span key={index}>{piece.text}</span>
        ) : piece.kind === 'added' ? (
          <ins key={index} className="rounded-sm bg-success-subtle text-text no-underline">
            {piece.text}
          </ins>
        ) : (
          <del key={index} className="rounded-sm bg-danger-subtle text-text">
            {piece.text}
          </del>
        ),
      )}
    </span>
  );
}

const textOf = (block: { type: string; content: unknown } | null): string => (block === null ? '' : plainText(block.content as Content));

/** The name of a block's type in the list of what changed. */
function typeName(t: TFunction, block: { type: string; props: unknown } | null): string {
  if (block === null) return t('documents.blocks.paragraph');
  if (block.type === 'heading') return t('documents.blocks.heading', { level: (block.props as { level?: number } | null)?.level ?? 1 });
  if (block.type === 'columns') return t('documents.blocks.columnsPlain');
  return t(`documents.blocks.${block.type}` as 'documents.blocks.paragraph');
}

export function ChangesView({ changes, columns, doc, onStale = () => undefined }: { changes: DocumentChanges; columns: 1 | 2; doc?: Document; onStale?: () => void }) {
  const { t } = useTranslation();
  if (changes.blocks.length === 0 && changes.title === null) return <EmptyState compact title={t('documents.history.nothing')} />;
  return (
    <ul className="flex flex-col gap-2" aria-label={t('documents.history.difference')}>
      {changes.title !== null && (
        <li className="rounded-md border border-border p-2 text-sm">
          <p className="text-xs text-text-muted">{t('documents.history.title')}</p>
          <Words before={changes.title.before} after={changes.title.after} />
        </li>
      )}
      {changes.blocks.map((one) => {
        const kind = changeKind(one);
        const before = textOf(one.before);
        const after = textOf(one.after);
        return (
          <li
            key={one.id}
            data-change={kind}
            className={cx('rounded-md border p-2 text-sm', kind === 'added' ? 'border-success bg-success-subtle' : kind === 'removed' ? 'border-danger bg-danger-subtle' : 'border-border bg-surface')}
          >
            <p className="mb-1 flex flex-wrap items-center gap-x-2 text-xs text-text-muted">
              <span className="font-semibold text-text">{t(`documents.history.kind_${kind}` as 'documents.history.kind_added')}</span>
              <span>{typeName(t, one.after ?? one.before)}</span>
              {one.by !== null && <span className="inline-flex items-center gap-1"><AuthorChip author={one.by} /></span>}
            </p>
            {kind === 'changed' && columns === 2 ? (
              <div className="grid grid-cols-2 gap-3">
                <div className="whitespace-pre-wrap text-text-muted">{before}</div>
                <div className="whitespace-pre-wrap">{after}</div>
              </div>
            ) : kind === 'changed' ? (
              <Words before={before} after={after} />
            ) : (
              <span className={cx('whitespace-pre-wrap', kind === 'removed' && 'line-through')}>{kind === 'removed' ? before : after}</span>
            )}
            {doc !== undefined && <PutBack key={`${one.before?.seq_updated ?? 0}:${one.after?.seq_updated ?? 0}`} doc={doc} change={one} changes={changes} onStale={onStale} />}
          </li>
        );
      })}
    </ul>
  );
}

function Detail({ doc, entry, onShow }: { doc: Document; entry: DocumentVersion; onShow: (version: number) => void }) {
  const { t } = useTranslation();
  const [columns, changeColumns] = useListChoice<'1' | '2'>(`document-history.${doc.id}`, 'difference', '1');
  const setColumns = (next: '1' | '2') => { changeColumns(next); };
  const [putting, setPutting] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const from = entry.version === null ? doc.version : entry.version - 1;
  const [reviewCurrent, setReviewCurrent] = useState(false);
  const changes = useQuery({ ...changesQuery(doc.id, String(from), reviewCurrent ? null : entry.version), staleTime: 0 });
  const canManage = holds(doc.access, 'doc.manage') && !doc.archived;
  const files = entry.changed.files ?? [];
  const hasBlocks = changes.data !== undefined && changes.data.blocks.length > 0;
  return (
    <section aria-label={t('documents.history.detail')} className="flex flex-col gap-3 border-t border-border pt-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        {hasBlocks && <SegmentedControl<'1' | '2'>
          label={t('documents.history.layout')}
          value={columns}
          onValueChange={setColumns}
          segments={[
            { value: '1', ...choiceOption(t, 'diffLayout.one'), icon: <Rows2 aria-hidden /> },
            { value: '2', ...choiceOption(t, 'diffLayout.two'), icon: <Columns2 aria-hidden /> },
          ]}
        />}
        <div className="flex gap-2">
          {entry.version !== null && (
            <Button size="sm" onClick={() => onShow(entry.version ?? 0)}>
              {t('documents.history.showThen')}
            </Button>
          )}
          {entry.version !== null && canManage && (
            <Button size="sm" variant="primary" onClick={() => setPutting(true)}>
              {t('documents.history.putBack')}
            </Button>
          )}
        </div>
      </div>
      {reviewCurrent && <p role="status" className="text-sm text-text-muted">{t('people.reviewCurrent')}</p>}
      <FileChanges files={files} />
      {changes.isPending ? <SkeletonRows rows={3} /> : changes.isError ? <ErrorState compact error={changes.error} onRetry={() => void changes.refetch()} /> : (files.length === 0 || changes.data.blocks.length > 0 || changes.data.title !== null) && <ChangesView key={`${reviewCurrent}:${changes.data.to}`} changes={changes.data} doc={doc} columns={columns === '2' ? 2 : 1} onStale={() => { refreshDocument(doc.id); setReviewCurrent(true); void changes.refetch(); }} />}
      <ConfirmDialog
        open={putting}
        onOpenChange={setPutting}
        title={t('documents.history.putBackTitle')}
        confirmLabel={t('documents.history.putBack')}
        pending={pending}
        error={error}
        onConfirm={() => {
          if (entry.version === null) return;
          setPending(true);
          setError(null);
          restoreVersion(doc.id, entry.version)
            .then((sync) => {
              // The page is what the version says again; the present one was kept as a version first.
              documentWriter.reset(doc.id, sync, sync.seq);
              setPutting(false);
            })
            .catch(setError)
            .finally(() => setPending(false));
        }}
      >
        {t('documents.history.putBackAbout', { title: entry.title })}
      </ConfirmDialog>
    </section>
  );
}

// ---------------------------------------------------------------- the drawer

export function HistoryDrawer({ doc, open, onOpenChange, onShow }: { doc: Document; open: boolean; onOpenChange: (open: boolean) => void; onShow?: (version: number) => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [author, setAuthor] = useListChoice<string>('document-history', 'author', 'all');
  const [chosen, setChosen] = useState<number | 'open' | null>(null);
  const say = (key: 'changed' | 'added' | 'removed', count: number): string => t(`documents.history.${key}` as 'documents.history.changed', { count });
  const sayFile = (kind: FileChange['kind'], count: number): string => t(FILE_COUNTS[kind], { count });
  const list = useInfiniteQuery({ ...versionsQuery(doc.id, author === 'all' ? null : author), enabled: open, staleTime: 0, refetchOnMount: 'always' });
  const entries = useMemo(() => list.data?.pages.flatMap((page) => page.items) ?? [], [list.data]);
  const authors = useMemo(() => {
    const seen = new Map<string, Author>();
    for (const entry of entries) for (const one of entry.authors) seen.set(authorKey(one), one);
    return [...seen];
  }, [entries]);
  const selected = entries.find((one) => (one.version === null ? chosen === 'open' : one.version === chosen)) ?? null;
  const numbered = entries.filter((one) => one.version !== null);
  const bytes = numbered.reduce((sum, one) => sum + one.bytes, 0);
  const oldest = numbered[numbered.length - 1];

  return (
    <Drawer
      open={open}
      onOpenChange={(next) => {
        // The drawer opens on the list each time, with no entry chosen.
        if (!next) setChosen(null);
        onOpenChange(next);
      }}
      side="right"
      title={t('documents.history.title', { title: doc.title })}
      className="w-xl!"
    >
      <div className="flex flex-col gap-3 overflow-y-auto p-4">
        <Select<string>
          label={t('documents.history.author')}
          value={author}
          onValueChange={(next) => {
            setAuthor(next);
            setChosen(null);
          }}
          options={[{ value: 'all', ...choiceOption(t, 'statusFilter.any'), description: t('documents.history.everybody') }, ...authors.map(([key, one]) => ({ value: key, ...choiceOption(t, 'dynamic.value', { name: authorName(one), context: t('documents.history.everybody') }) }))]}
        />
        {list.isPending ? (
          <SkeletonRows rows={4} />
        ) : list.isError ? (
          <ErrorState compact error={list.error} onRetry={() => void list.refetch()} />
        ) : entries.length === 0 ? (
          <EmptyState compact title={t('documents.history.empty')} />
        ) : (
          <ul className="flex flex-col gap-1" aria-label={t('documents.history.entries')}>
            {entries.map((entry) => {
              const key = entryKey(entry);
              return (
                <li key={key}>
                  <button
                    type="button"
                    aria-pressed={chosen === key}
                    data-version={key}
                    className={cx('flex w-full flex-col gap-1 rounded-md border px-3 py-2 text-left text-sm', chosen === key ? 'border-accent bg-accent-subtle' : 'border-transparent hover:bg-bg-muted')}
                    onClick={() => setChosen(chosen === key ? null : key)}
                  >
                    <span className="flex flex-wrap items-baseline justify-between gap-2">
                      <span className="font-medium text-text">{entry.version === null ? t('documents.history.openSpan') : <RelativeTime at={entry.to} />}</span>
                      <span className="inline-flex flex-wrap items-center gap-2 text-text-muted">{entry.authors.map((author, index) => <AuthorChip key={index} author={author} />)}</span>
                    </span>
                    <span className="text-text-muted">{summary(say, entry.changed, sayFile)}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
        <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : null} onMore={() => void list.fetchNextPage()} />
        {selected !== null && (
          <Detail
            doc={doc}
            entry={selected}
            onShow={(version) => {
              setChosen(null);
              onShow?.(version);
            }}
          />
        )}
        <p className="text-xs text-text-muted">
          {numbered.length === 0
            ? t('documents.history.footNone')
            : t('documents.history.foot', { count: numbered.length, size: formatBytes(bytes, locale), since: oldest === undefined ? '' : new Date(oldest.to).toLocaleDateString(locale, { day: 'numeric', month: 'long' }) })}
        </p>
      </div>
    </Drawer>
  );
}
