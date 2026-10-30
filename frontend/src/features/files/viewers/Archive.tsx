/** An archive listed as a tree: what it holds, with sizes, and one entry opened or downloaded.
 *
 * The listing is read where the file lies, directory by directory as they are opened, and nothing is
 * ever unpacked. A text or an image inside opens in its own viewer, read-only; an entry too large to
 * be read here, or one in an archive that asks for a password, is listed and not opened.
 */
import { useQuery } from '@tanstack/react-query';
import { ChevronRight, File as FileIcon, Folder, Link2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { archiveQuery, readArchiveEntry } from '@/api/workspace';
import type { ArchiveEntry } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { cx } from '@/ui/cx';
import { CodeView } from '@/ui/CodeView';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';

import { extensionOf, languageOf } from '../languages';
import { type ViewerProps } from '../viewers';
import { useObjectUrl } from './Image';
import { sourceArchive, sourceArchiveEntry, useFileSource } from '../source';

const baseName = (path: string): string => path.replace(/\/+$/, '').slice(path.replace(/\/+$/, '').lastIndexOf('/') + 1);
const IMAGE_TYPES: Record<string, string> = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml', bmp: 'image/bmp', ico: 'image/x-icon', avif: 'image/avif' };

interface Shared {
  door: ViewerProps['door'];
  path: string;
  encrypted: boolean;
  chosen: string | null;
  choose: (entry: string) => void;
}

function Listing({ shared, inside, depth }: { shared: Shared; inside: string; depth: number }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const provided = useFileSource();
  const source = provided?.workspace === undefined ? provided : null;
  const listing = useQuery(source === null ? archiveQuery(shared.door, shared.path, inside) : {
    queryKey: ['workspace-archive', source.url, shared.path, inside] as const, queryFn: ({ signal }) => sourceArchive(source, inside, signal), retry: false,
  });
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  if (listing.error !== null) return <ErrorState compact error={listing.error} onRetry={() => void listing.refetch()} />;
  if (listing.data === undefined) return <SkeletonRows rows={3} />;
  const rows = listing.data.entries;
  return (
    <ul role={depth === 0 ? 'tree' : 'group'} aria-label={depth === 0 ? t('files.archive.label') : undefined}>
      {rows.map((entry: ArchiveEntry) => {
        const name = baseName(entry.path);
        const isDir = entry.kind === 'dir';
        const expanded = open.has(entry.path);
        return (
          <li key={entry.path} role="treeitem" aria-expanded={isDir ? expanded : undefined} aria-selected={shared.chosen === entry.path}>
            <button
              type="button"
              disabled={!isDir && entry.kind !== 'file'}
              onClick={() => {
                if (isDir) setOpen((before) => new Set(expanded ? [...before].filter((one) => one !== entry.path) : [...before, entry.path]));
                else shared.choose(entry.path);
              }}
              className={cx(
                'focus-inset flex h-(--row-height) w-full items-center gap-1 pr-3 text-left text-base hover:bg-bg-muted',
                shared.chosen === entry.path && 'bg-accent-subtle',
              )}
              style={{ paddingInlineStart: `calc(var(--spacing-4) * ${depth} + var(--spacing-2))` }}
            >
              <span className="flex size-4 shrink-0 items-center justify-center text-text-subtle">{isDir ? <ChevronRight className={cx('transition-transform', expanded && 'rotate-90')} aria-hidden /> : null}</span>
              {isDir ? <Folder className="shrink-0 text-text-muted" aria-hidden /> : entry.kind === 'link' ? <Link2 className="shrink-0 text-text-muted" aria-hidden /> : <FileIcon className="shrink-0 text-text-muted" aria-hidden />}
              <span className="min-w-0 flex-1 truncate">{name}</span>
              {entry.size !== null && !isDir && <span className="text-xs text-text-muted tabular-nums">{formatBytes(entry.size, locale)}</span>}
            </button>
            {isDir && expanded && <Listing shared={shared} inside={entry.path} depth={depth + 1} />}
          </li>
        );
      })}
      {listing.data.truncated && depth >= 0 && <li className="px-3 py-1 text-xs text-text-muted">{t('files.archive.truncated', { count: rows.length })}</li>}
    </ul>
  );
}

function Entry({ door, path, entry }: { door: ViewerProps['door']; path: string; entry: string }) {
  const { t } = useTranslation();
  const provided = useFileSource();
  const source = provided?.workspace === undefined ? provided : null;
  const query = useQuery({ queryKey: ['archive-entry', source?.url ?? door.environmentId, path, entry], queryFn: ({ signal }) => source === null ? readArchiveEntry(door, path, entry, signal) : sourceArchiveEntry(source, entry, signal), staleTime: Number.POSITIVE_INFINITY, retry: false });
  const extension = extensionOf(entry);
  const imageType = IMAGE_TYPES[extension];
  const url = useObjectUrl(imageType === undefined ? null : (query.data ?? null), imageType ?? 'image/png');
  if (query.error !== null) return <ErrorState compact className="h-full" title={t('files.archive.tooLarge')} error={query.error} />;
  if (query.data === undefined) return <SkeletonRows rows={4} />;
  if (imageType !== undefined) return url === null ? <SkeletonRows rows={4} /> : <img src={url} alt={entry} className="mx-auto max-h-full max-w-full object-contain p-2" />;
  const text = new TextDecoder('utf-8', { fatal: true });
  let shown: string;
  try {
    shown = text.decode(query.data);
  } catch {
    return <ErrorState compact className="h-full" title={t('files.archive.noPreview')} error={null} />;
  }
  const language = languageOf(entry, shown.slice(0, 200).split('\n', 1)[0] ?? null);
  return (
    <CodeView
      className="h-full"
      text={shown}
      version={`${entry}:${query.dataUpdatedAt}`}
      language={language === null ? null : language.load}
      languageKey={language?.id ?? null}
      readOnly
      label={t('files.archive.entryLabel', { name: baseName(entry) })}
    />
  );
}

export default function ArchiveViewer({ door, path, head }: ViewerProps) {
  const { t } = useTranslation();
  const provided = useFileSource();
  const source = provided?.workspace === undefined ? provided : null;
  const [chosen, setChosen] = useState<string | null>(null);
  const top = useQuery(source === null ? archiveQuery(door, path, '') : {
    queryKey: ['workspace-archive', source.url, path, ''] as const, queryFn: ({ signal }) => sourceArchive(source, '', signal), retry: false,
  });
  const encrypted = top.data?.encrypted === true;
  const shared: Shared = { door, path, encrypted, chosen, choose: (entry) => (encrypted ? undefined : setChosen(entry)) };
  return (
    <div className="@container flex h-full min-h-0 flex-col" data-viewer="archive">
      <div className="flex items-center gap-2 border-b border-border px-3 py-1 text-sm text-text-muted">
        <span className="min-w-0 flex-1 truncate">{top.data === undefined ? head.stat.name : t('files.archive.kind', { format: top.data.format, compression: top.data.compression ?? t('files.archive.none') })}</span>
      </div>
      {encrypted && (
        <InlineAlert tone="warning" className="m-2">
          {t('files.archive.encrypted')}
        </InlineAlert>
      )}
      <div className={cx('grid min-h-0 flex-1 gap-px bg-border', chosen === null ? 'grid-cols-1' : 'grid-cols-1 grid-rows-2 @3xl:grid-cols-2 @3xl:grid-rows-1')}>
        <div className="min-h-0 overflow-auto bg-surface">
          <Listing shared={shared} inside="" depth={0} />
        </div>
        {chosen !== null && (
          <div className="min-h-0 min-w-0 overflow-auto bg-surface" data-testid="archive-entry">
            <Entry door={door} path={path} entry={chosen} />
          </div>
        )}
      </div>
    </div>
  );
}
