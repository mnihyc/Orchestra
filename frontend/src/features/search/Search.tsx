/** Search results and permalinks.
 *
 * The page of `/search`: one field, filters on request, and cards in the order streamed by the server.
 * The address carries the search; this device remembers the chosen sort and filters without its text.
 * A search that is too short, one that finds nothing, and one the server does not answer each say so in
 * words and say what to do about it. A result opens the thing it is part of, at the place where it was said.
 */
import { useQuery } from '@tanstack/react-query';
import { Search as SearchIcon, SearchX, SlidersHorizontal } from 'lucide-react';
import { Fragment, type SyntheticEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { orgsQuery } from '@/api/nodes';
import { ApiError } from '@/api/client';
import { tooShort } from '@/api/search';
import { Filters } from '@/features/search/Filters';
import { Hit } from '@/features/search/Hit';
import { SEARCH_TABS, filtersOf, narrowing, readParams, restoredView, savedView, tabOf, withTab, type SearchParams } from '@/features/search/params';
import { useSearchStream } from '@/features/search/useSearchStream';
import { ActiveFilters } from '@/features/search/ActiveFilters';
import { useFilterValues, type PersonFilter } from '@/features/search/filterValues';
import { Hint } from '@/features/help/Hint';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { labelsOfHits } from '@/features/search/labels';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { useDebounced } from '@/lib/debounce';
import { useViewChoice } from '@/lib/viewChoices';
import { useScreenHeading } from '@/shell/screen';
import { Page, PageHeader } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { Tooltip } from '@/ui/Tooltip';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { FilterBar } from '@/ui/FilterBar';
import { Drawer } from '@/ui/Drawer';
import { Tabs } from '@/ui/Tabs';


/** How long the field rests before the address is changed: the keys of one word are one search. */
const SEARCH_PAUSE_MS = 400;

/** The words that were written into the address by this page last, so that its own change is told from a change made elsewhere. */
let writtenByPage: string | undefined;

export interface SearchPageProps {
  params: SearchParams;
  onParams: (next: SearchParams) => void;
}

export function SearchPage({ params, onParams }: SearchPageProps) {
  const [kept, keep] = useViewChoice<string>('search', 'filters', '');
  const [restored, setRestored] = useState(() => restoredView(params, kept));
  const [resumed, setResumed] = useState(restored !== undefined);
  if (restored !== undefined && savedView(params) === savedView(restored) && params.q === restored.q) setRestored(undefined);
  const shown = restored ?? params;
  const view = savedView(shown);
  useEffect(() => { keep(view); }, [keep, view]);
  useEffect(() => {
    if (restored === undefined) return;
    onParams(restored);
  }, [onParams, restored]);
  return <SearchResults params={shown} onParams={(next) => {
    if (savedView(next) !== view) setResumed(false);
    setRestored(undefined);
    onParams(next);
  }} onUnavailable={(error, person) => {
    if (!resumed || !(error instanceof ApiError) || (error.status !== 403 && error.status !== 404)) return false;
    if (person !== undefined) {
      if (shown[person] === undefined) return false;
      const next = readParams({ ...shown, [person]: undefined });
      setRestored(undefined); keep(savedView(next)); onParams(next);
      return true;
    }
    if (shown.scope === undefined && shown.chat_id === undefined && shown.document_id === undefined) return false;
    setResumed(false); setRestored(undefined); keep('{}');
    onParams(shown.q === undefined ? {} : {q: shown.q});
    return true;
  }} />;
}

function SearchResults({ params, onParams, onUnavailable }: SearchPageProps & {onUnavailable: (error: unknown, person?: PersonFilter) => boolean}) {
  const { t } = useTranslation();
  const [filtersOpen, setFiltersOpen] = useState(false);
  useScreenHeading({ title: t('screens.search.title'), icon: SearchIcon });

  const [typedAt, setTypedAt] = useState(0);
  const [immediate, setImmediate] = useState(true);
  const [submitted, setSubmitted] = useState(0);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [details, setDetails] = useState(false);
  const locale = useAppearance((state) => state.locale);
  const [text, setText] = useState(params.q ?? '');
  const [seen, setSeen] = useState(params.q);
  const values = useFilterValues(params);
  const { owner: ownerError, created_by: creatorError, edited_by: editorError, author: authorError } = values.errors;
  useEffect(() => {
    for (const [person, error] of [['owner', ownerError], ['created_by', creatorError], ['edited_by', editorError], ['author', authorError]] as const) {
      if (error !== null && onUnavailable(error, person)) break;
    }
  }, [ownerError, creatorError, editorError, authorError, onUnavailable]);
  // The address changed from outside (the finder led here, the back button): the field follows it.
  if (params.q !== seen) {
    setSeen(params.q);
    if (params.q !== writtenByPage) setText(params.q ?? '');
  }

  const rested = useDebounced(text, SEARCH_PAUSE_MS);
  const latest = useRef(params);
  useEffect(() => {
    latest.current = params;
  });
  useEffect(() => {
    const words = rested.trim();
    if (words === (latest.current.q ?? '')) return;
    writtenByPage = words === '' ? undefined : words;
    onParams({ ...latest.current, ...(words === '' ? { q: undefined } : { q: words }) });
    // The address is only written when the field has rested on new words.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rested]);

  const filters = filtersOf(params);
  const narrow = params.scope !== undefined;
  const length = tooShort(params.q ?? '', { narrow, streamed: true });
  const search = useSearchStream(filters, length === null && text.trim() === (params.q ?? '').trim(), immediate, submitted, typedAt, onUnavailable);
  const orgs = useQuery(orgsQuery).data;
  const slugOfOrg = useCallback((id: string) => orgs?.find((org) => org.id === id)?.slug ?? null, [orgs]);
  const hits = search.ids.flatMap((id) => search.hits[id] === undefined ? [] : [search.hits[id]]);
  useEffect(() => {
    const watch = new MutationObserver(() => setDetails(document.querySelector('[data-search-result], [data-reference-card], [data-person-card], [data-search-rows] [data-state="open"], [data-search-rows] [aria-expanded="true"]') !== null));
    watch.observe(document.body, {childList: true, subtree: true, attributes: true, attributeFilter: ['data-state', 'aria-expanded']});
    return () => watch.disconnect();
  }, []);
  const block = search.block;
  useEffect(() => {block(hovered || focused || details);}, [block, hovered, focused, details]);
  const known = useKnownPlaces();
  const labels = useMemo(() => labelsOfHits(hits, known), [hits, known]);

  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    const words = text.trim();
    setImmediate(true); setSubmitted((value) => value + 1);
    writtenByPage = words === '' ? undefined : words;
    onParams({ ...params, ...(words === '' ? { q: undefined } : { q: words }) });
  };

  const meaningReason = search.reasons.meaning === undefined || search.reasons.meaning === null ? '' : t(`search.meaningReasons.${search.reasons.meaning}`, {defaultValue: search.reasons.meaning});
  const indexPaused = search.reasons.meaning === 'provider_disabled' || search.reasons.meaning === 'provider_removed';
  const error = search.error ?? (search.unavailable.words === true ? new Error(t('search.wordsUnavailable')) : null);
  const activeFilters = narrowing(params);
  const filtersLabel = activeFilters === 0 ? t('search.filters.button') : t('search.filters.active', { count: activeFilters });

  const resultPanel = <>
    <ActiveFilters params={params} onParams={onParams} values={values} />
    <section aria-label={t('search.results')} aria-busy={search.unfinished} className="flex min-w-0 flex-col gap-3">
          {length === 'none' && (
            <EmptyState icon={<SearchIcon aria-hidden />} title={t('search.state.none')} />
          )}
          {length === 'short' && <EmptyState icon={<SearchIcon aria-hidden />} title={t('search.state.short')} description={t('search.state.shortAbout')} />}
          {length === 'indexed' && <EmptyState icon={<SearchIcon aria-hidden />} title={t('search.state.indexed')} description={t('search.state.indexedAbout')} />}

          {length === null && error !== null && (
            <ErrorAlert
              error={error}
              message={search.unavailable.words === true ? t('search.wordsUnavailable') : undefined}
              action={
                <Button size="sm" onClick={() => setSubmitted((value) => value + 1)}>
                  {t('common.tryAgain')}
                </Button>
              }
            />
          )}
          {length === null && !search.unfinished && error === null && hits.length === 0 && (
            <EmptyState
              icon={<SearchX aria-hidden />}
              title={t('search.state.nothing', { query: params.q ?? '' })}
              description={narrowing(params) > 0 ? t('search.state.nothingNarrow') : t('search.state.nothingAbout')}
            />
          )}

          {length === null && <p role="status" className="min-h-5 text-sm text-text-muted tabular-nums">
            {search.unfinished ? t('search.searching', {count: search.total, number: formatNumber(search.total, locale)}) : t('search.total', {count: search.total, number: formatNumber(search.total, locale)})}
            {search.unavailable.meaning === true && <> · <MeaningStatus label={t('search.meaningUnavailable')} reason={meaningReason} /></>}
            {search.unavailable.meaning !== true && indexPaused && <> · <MeaningStatus label={t('search.indexPaused')} reason={meaningReason} /></>}
            {search.unavailable.meaning !== true && !indexPaused && search.reasons.meaning !== undefined && search.reasons.meaning !== null && <span> · {t(`search.wordsOnly.${search.reasons.meaning}`, {defaultValue: search.reasons.meaning})}</span>}
          </p>}
          {hits.length > 0 && <>
            <ul data-search-rows className="flex flex-col gap-3 motion-safe:transition-[opacity] motion-safe:duration-150"
              onPointerOver={(event) => setHovered((event.target as HTMLElement).closest('li') !== null)}
              onPointerOut={(event) => setHovered(event.relatedTarget instanceof HTMLElement && event.currentTarget.contains(event.relatedTarget) && event.relatedTarget.closest('li') !== null)}
              onPointerLeave={() => setHovered(false)} onFocusCapture={() => setFocused(true)}
              onBlurCapture={(event) => {if (!event.currentTarget.contains(event.relatedTarget)) setFocused(false);}}>
              {search.ids.map((id, index) => {
                const hit = search.hits[id];
                if (hit === undefined) return null;
                const divider = search.settled && search.membership[id] === 'meaning' && (index === 0 || search.membership[search.ids[index - 1] ?? ''] !== 'meaning');
                return <Fragment key={id}>
                  {divider && <li role="separator" className="border-t border-border pt-2 text-sm text-text-muted">{t('search.similarMeaning')}</li>}
                  <Hit hit={hit} slugOfOrg={slugOfOrg} labels={labels} foundBy={search.membership[id]} matchField={search.fields[id]} />
                </Fragment>;
              })}
            </ul>
            <div className="flex justify-center border-t border-border py-2">
              {search.hasMore ? <Button variant="secondary" disabled={search.unfinished && search.expanded} onClick={search.more}>{t('search.more')}</Button>
                : !search.unfinished && <p className="text-sm text-text-muted">{t('search.end')}</p>}
            </div>
          </>}

    </section>
  </>;

  return <Page>
    <PageHeader title={t('screens.search.title')} actions={<Hint id="search-results" />} />
    <form role="search" aria-label={t('search.title')} onSubmit={submit}>
      <FilterBar search={<Input label={t('search.field.label')} hideLabel value={text}
        onChange={(event) => {setTypedAt(Date.now()); setImmediate(false); setText(event.target.value);}}
        placeholder={t('search.field.placeholder')} autoComplete="off" spellCheck={false} fieldClassName="flex-1" data-autofocus />}
        action={<Button className="min-[1024px]:hidden" icon={<SlidersHorizontal aria-hidden />} onClick={() => setFiltersOpen(true)}>{filtersLabel}</Button>} />
    </form>
    <div className="grid min-w-0 gap-6 min-[1024px]:grid-cols-[15rem_minmax(0,1fr)]">
      <aside className="hidden min-w-0 min-[1024px]:block" aria-label={t('search.filters.title')}>
        <Filters params={params} onParams={onParams} values={values} />
      </aside>
      <Tabs label={t('search.tabs.label')} value={tabOf(params)} onValueChange={(tab) => onParams(withTab(params, tab))}
        tabs={SEARCH_TABS.map((tab) => ({ value: tab, label: t(`search.tabs.${tab}` as const), panel: resultPanel }))} />
    </div>
    <Drawer open={filtersOpen} onOpenChange={setFiltersOpen} title={t('search.filters.title')} side="bottom">
      <div className="p-4"><Filters params={params} onParams={onParams} values={values} /></div>
    </Drawer>
  </Page>;
}

/** A source or index reason is available from the same quiet label by pointer, focus or tap. */
function MeaningStatus({reason,label}:{reason:string;label:string}) {
  const [open,setOpen]=useState(false);
  return <Tooltip content={reason} open={open} onOpenChange={setOpen}>
    <button type="button" title={reason} className="focus-inset rounded-sm text-left" onClick={()=>setOpen((before)=>!before)}>{label}</button>
  </Tooltip>;
}
