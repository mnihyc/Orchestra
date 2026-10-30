/** The search inside files: the lines that hold a word or a pattern, file by file, under the directory chosen.
 *
 * One field does two searches. By name lists the paths that hold the text, from the last list of the
 * workspace's tree; in files lists the lines that hold it, read from the environment as it is now, so it
 * needs the environment online and says so when it is not. A hit opens its file at that line.
 */
import { useQuery } from '@tanstack/react-query';
import { CaseSensitive, Folder, FolderOpen, Regex, Search, X } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { ApiError } from '@/api/client';
import { type ContentMatches, type Door, searchContent, searchNames, workspaceKeys } from '@/api/workspace';
import { useDebounced } from '@/lib/debounce';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { SkeletonRows } from '@/ui/Skeleton';
import { FileChip } from './FileChip';
import { workspaceSource } from './source';

/** The fewest characters a search is made of: the API refuses less. */
export const SEARCH_LEAST_CHARS = 2;

export type SearchMode = 'names' | 'content';

export interface SearchState {
  text: string;
  mode: SearchMode;
  regex: boolean;
  caseSensitive: boolean;
  /** The directory the lines are looked for in; the thread's own when empty. */
  under: string;
}

export const NO_SEARCH: SearchState = { text: '', mode: 'names', regex: false, caseSensitive: false, under: '' };

export interface SearchBoxProps {
  state: SearchState;
  onChange: (state: SearchState) => void;
  /** Whether the workspace is searched by name at all: it is not where it is too large to index. */
  namesAvailable: boolean;
  /** The directory the thread's own is, for the label of the scope. */
  directory: string;
  autoFocus?: boolean;
}

export function SearchBox({ state, onChange, namesAvailable, directory, autoFocus }: SearchBoxProps) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-1 px-2 pb-1">
      <Input
        autoFocus={autoFocus}
        label={t('files.search.label')}
        hideLabel
        placeholder={state.mode === 'names' ? t('files.search.placeholderNames') : t('files.search.placeholderContent')}
        value={state.text}
        onChange={(event) => onChange({ ...state, text: event.target.value })}
        onKeyDown={(event) => {
          if (event.key === 'Escape') onChange({ ...state, text: '' });
        }}
        trailing={
          state.text === '' ? (
            <Search className="text-text-subtle" aria-hidden />
          ) : (
            <IconButton size="sm" label={t('files.search.clear')} icon={<X aria-hidden />} onClick={() => onChange({ ...state, text: '' })} />
          )
        }
      />
      <div className="flex flex-wrap items-center gap-1">
        <SegmentedControl<SearchMode>
          size="sm"
          label={t('files.search.mode')}
          value={state.mode}
          onValueChange={(mode) => onChange({ ...state, mode })}
          segments={[
            { value: 'names', ...choiceOption(t, 'fileSearch.names'), disabled: !namesAvailable },
            { value: 'content', ...choiceOption(t, 'fileSearch.content') },
          ]}
        />
        {state.mode === 'content' && (
          <>
            <IconButton size="sm" label={t('files.search.regex')} pressed={state.regex} icon={<Regex aria-hidden />} onClick={() => onChange({ ...state, regex: !state.regex })} />
            <IconButton size="sm" label={t('files.search.case')} pressed={state.caseSensitive} icon={<CaseSensitive aria-hidden />} onClick={() => onChange({ ...state, caseSensitive: !state.caseSensitive })} />
          </>
        )}
      </div>
      {state.mode === 'content' && (
        <p className="flex items-center gap-1 text-xs text-text-muted">
          <Folder className="shrink-0" aria-hidden />
          <span className="min-w-0 flex-1 truncate">{t('files.search.under', { directory: state.under === '' ? (directory === '' ? t('files.search.wholeWorkspace') : directory) : state.under })}</span>
          {state.under !== '' && (
            <button type="button" className="focus-inset rounded-md px-1 text-accent-text underline" onClick={() => onChange({ ...state, under: '' })}>
              {t('files.search.reset')}
            </button>
          )}
        </p>
      )}
    </div>
  );
}

/** Splits a line into the parts that match and the parts that do not, by the ranges the API gives. */
function marked(text: string, ranges: readonly (readonly [number, number])[]): { text: string; hit: boolean }[] {
  const parts: { text: string; hit: boolean }[] = [];
  let at = 0;
  for (const [from, to] of [...ranges].sort((one, other) => one[0] - other[0])) {
    if (from < at || from >= text.length) continue;
    if (from > at) parts.push({ text: text.slice(at, from), hit: false });
    parts.push({ text: text.slice(from, Math.min(to, text.length)), hit: true });
    at = Math.min(to, text.length);
  }
  if (at < text.length) parts.push({ text: text.slice(at), hit: false });
  return parts;
}

export interface SearchResultsProps {
  door: Door;
  state: SearchState;
  /** The directory a search of lines stays in when none was chosen. */
  directory: string;
  onOpenPath: (path: string, line: number | null) => void;
  onOpenFolder: (path: string) => void;
}

export function SearchResults({ door, state, directory, onOpenPath, onOpenFolder }: SearchResultsProps) {
  const { t } = useTranslation();
  const text = useDebounced(state.text.trim());
  const ready = text.length >= SEARCH_LEAST_CHARS;
  const under = state.under === '' ? directory : state.under;
  const names = useQuery({
    queryKey: workspaceKeys.search(door, text, 'names'),
    queryFn: ({ signal }) => searchNames(door, text, signal),
    enabled: ready && state.mode === 'names',
    staleTime: 10_000,
    retry: false,
  });
  const content = useQuery({
    queryKey: [...workspaceKeys.search(door, text, 'content'), under, state.regex, state.caseSensitive] as const,
    queryFn: ({ signal }) => searchContent(door, text, { under, regex: state.regex, caseSensitive: state.caseSensitive }, signal),
    enabled: ready && state.mode === 'content',
    staleTime: 10_000,
    retry: false,
  });

  const query = state.mode === 'names' ? names : content;
  const grouped = useMemo(() => {
    const data = content.data;
    const groups = new Map<string, ContentMatches['matches']>();
    for (const match of data?.matches ?? []) groups.set(match.path, [...(groups.get(match.path) ?? []), match]);
    return [...groups];
  }, [content.data]);

  if (!ready) return <EmptyState compact className="flex-1" icon={<Search aria-hidden />} title={t('files.search.tooShort', { count: SEARCH_LEAST_CHARS })} />;
  if (query.error !== null) {
    if (query.error instanceof ApiError && query.error.is('workspace.not_indexed')) {
      return <EmptyState compact className="flex-1" icon={<Search aria-hidden />} title={t('files.search.notIndexed')} />;
    }
    if (query.error instanceof ApiError && query.error.is('source.offline')) {
      return <EmptyState compact className="flex-1" icon={<Search aria-hidden />} title={t('files.search.offline')} />;
    }
    return <ErrorState compact className="flex-1" error={query.error} onRetry={() => void query.refetch()} />;
  }
  if (query.isPending) return <SkeletonRows rows={5} />;

  if (state.mode === 'names') {
    const entries = names.data ?? [];
    if (entries.length === 0) return <EmptyState compact className="flex-1" icon={<Search aria-hidden />} title={t('files.search.none')} />;
    return (
      <ul aria-label={t('files.search.results')} className="min-h-0 flex-1 overflow-auto" data-testid="search-results">
        {entries.map((entry) => (
          <li key={entry.path} className="flex items-center">
            {entry.type !== 'dir' ? <span className="min-w-0 flex-1 px-3 py-1"><FileChip source={workspaceSource(door, entry.path, entry.path, entry.size)} presentation={{ place: 'compact' }} bare onOpen={() => onOpenPath(entry.path, null)} /></span> : <button
              type="button"
              className="focus-inset flex min-h-(--row-height) min-w-0 flex-1 items-center gap-2 px-3 text-left text-base hover:bg-bg-muted"
              onClick={() => onOpenFolder(entry.path)}
            >
              <Folder className="shrink-0 text-accent-text" aria-hidden />
              <span className="min-w-0 flex-1 truncate" title={entry.path}>
                {entry.path}
              </span>
            </button>}
            {entry.type !== 'dir' && <IconButton size="sm" label={t('files.pages.openParent')} icon={<FolderOpen aria-hidden />} onClick={() => onOpenFolder(entry.path.includes('/') ? entry.path.slice(0, entry.path.lastIndexOf('/')) : '')} />}
          </li>
        ))}
      </ul>
    );
  }

  const data = content.data;
  if (data === undefined || data.matches.length === 0) return <EmptyState compact className="flex-1" icon={<Search aria-hidden />} title={t('files.search.none')} />;
  return (
    <div className="min-h-0 flex-1 overflow-auto" data-testid="search-results">
      <p className="px-3 py-1 text-xs text-text-muted" role="status">
        {t('files.search.summary', { count: data.matches.length, files: grouped.length, searched: data.searched })}
        {data.truncated && ` ${t('files.search.cut')}`}
      </p>
      <ul aria-label={t('files.search.results')}>
        {grouped.map(([path, matches]) => (
          <li key={path}>
            <div className="truncate bg-bg-subtle px-3 py-1 font-mono text-xs text-text-muted"><FileChip source={workspaceSource(door, path, path)} presentation={{ place: 'compact' }} bare onOpen={() => onOpenPath(path, matches[0]?.line ?? null)} /></div>
            <ul>
              {matches.map((match, index) => (
                <li key={`${match.line}.${index}`}>
                  <button
                    type="button"
                    className="focus-inset flex w-full items-baseline gap-2 px-3 py-1 text-left font-mono text-sm hover:bg-bg-muted"
                    onClick={() => onOpenPath(match.path, match.line)}
                  >
                    <span className="w-8 shrink-0 text-right text-xs text-text-muted tabular-nums">{match.line}</span>
                    <span className="min-w-0 flex-1 truncate">
                      {marked(match.text, match.ranges).map((part, at) => (
                        <span key={at} className={cx(part.hit && 'rounded-sm bg-warning-subtle font-semibold text-text')}>
                          {part.text}
                        </span>
                      ))}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </div>
  );
}
