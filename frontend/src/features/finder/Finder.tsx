/** The finder: one field for everything that has a name, every setting, every row of a page, every command and the help; its groups, the prefixes that keep to one, and what Enter does in each.
 *
 * It opens over the page from its key or the magnifier in the rail, empty or with a sign that keeps it
 * to one kind of answer. What is typed is looked for in what the page holds at once and, 150 ms after
 * the last key, among the names the server may show. The arrow keys walk the rows; Enter opens the one
 * chosen here, Control or Command with Enter in a new browser tab, Alt with Enter beside where
 * something can be shown beside. A row that the reader may not use is listed and says why.
 */
import { ThreadStateIcon } from '@/ui/ThreadState';
import type { ThreadDisplay } from '@/generated/api-types';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowRight, MessagesSquare, Star } from 'lucide-react';
import { type KeyboardEvent, type ReactNode, useCallback, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { orgsQuery } from '@/api/nodes';
import { recordItemVisit, starItem, unstarItem } from '@/api/personal';
import { SEARCH_MIN_CHARS, searchPreviewQuery } from '@/api/search';
import type { OpenHow } from '@/features/finder/commands';
import { Card } from '@/features/finder/Card';
import { TagProjects } from '@/features/finder/TagProjects';
import { destinationOf } from '@/features/finder/destinations';
import { Marked } from '@/features/finder/Marked';
import { MODES, type Mode, parseField, SIGNS, withMode } from '@/features/finder/modes';
import { assemble, type FinderRow, type GroupId, type Scored } from '@/features/finder/rows';
import { closeFinder, useFinder } from '@/features/finder/store';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { FilterBar } from '@/ui/FilterBar';
import { useAnswers, useEnv } from '@/features/finder/sources';
import { useHelpOpener } from '@/features/help/open';
import { destinationOfHit, excerptOf, wayOfHit } from '@/features/search/hits';
import { openResult, targetOfHit } from '@/features/search/ResultView';
import { WorkbenchChoiceRow } from '@/features/workbench/OpenViews';
import { labelsAlongPath, labelsOfHits } from '@/features/search/labels';
import type { NameHit } from '@/generated/api-types';
import { useNodeColor, useNodeOwnColor } from '@/features/navigation/places';
import { isApplePlatform } from '@/lib/shortcuts';
import { cx } from '@/ui/cx';
import { Kbd } from '@/ui/Kbd';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { NodeIcon, type NodeIconKind } from '@/ui/NodeIcon';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { Palette, PaletteEmpty, PaletteGroup, PaletteInput, PaletteItem, PaletteList } from '@/ui/Palette';
import { SkeletonRows } from '@/ui/Skeleton';
import { RelativeTime } from '@/ui/RelativeTime';
import { FileChip } from '@/features/files/FileChip';
import { FileOverlay } from '@/features/files/FileOverlay';
import type { FileSource } from '@/features/files/source';
import { filesOfHit } from '@/features/files/MessageFiles';
import { markedRanges } from './ranking';

/** The keys that walk the rows: once the reader used one, the finder stops choosing the first row by itself. */
const WALKING = new Set(['ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown']);

/** Where a key or a click asks for a row to open. */
function howOf(event: { ctrlKey: boolean; metaKey: boolean; altKey: boolean }, apple = isApplePlatform()): OpenHow {
  if (apple ? event.metaKey : event.ctrlKey) return 'tab';
  if (event.altKey) return 'beside';
  return 'here';
}

/** Bootstrap owns shell metadata; an explicitly opened finder can search before that metadata arrives. */
export function Finder({ ready = true }: { ready?: boolean }) {
  const { open, opened, seed, commandPrefix } = useFinder();
  // Every opening begins anew: the field, the choice, the card, and what was asked for.
  return <Session key={opened} open={open} seed={seed} commandPrefix={commandPrefix} ready={ready} />;
}

function Session({ open, seed, commandPrefix, ready }: { open: boolean; seed: string; commandPrefix: string | null; ready: boolean }) {
  const participating = useFinder((state) => state.participating);
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const openHelp = useHelpOpener();
  const orgs = useQuery({ ...orgsQuery, enabled: ready }).data;

  const [text, setText] = useState(seed);
  const [card, setCard] = useState<NameHit | null>(null);
  const [file, setFile] = useState<FileSource | null>(null);
  const [tagChoice, setTagChoice] = useState<NameHit | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<GroupId>>(new Set());
  // The row the reader walked to; before they walk, the first row is the chosen one.
  const [walkedTo, setWalkedTo] = useState<string | null>(null);
  const walked = useRef(false);
  const howRef = useRef<OpenHow>('here');
  const activated = useRef(false);
  const field = parseField(text);
  const { query } = field;
  const mode = commandPrefix === null ? field.mode : 'commands';
  const viewsOnly = commandPrefix === 'view.open.';

  const showCard = useCallback((hit: NameHit) => {
    if (hit.kind === 'tag' && (hit.tag ?? null) !== null && hit.path[0] !== undefined) {
      closeFinder();
      setTagChoice(hit);
    } else setCard(hit);
  }, []);
  const env = useEnv(showCard, ready);

  // What the words typed lead to besides names: the search of what was said.
  const letters = Array.from(query).length;
  const wantsHits = open && mode === 'conversations' && letters >= SEARCH_MIN_CHARS;
  const preview = useQuery({ ...searchPreviewQuery({ q: query, participating }), enabled: wantsHits }).data;
  const slugOfOrg = useCallback((id: string) => orgs?.find((org) => org.id === id)?.slug ?? null, [orgs]);

  const searchRow = useMemo<Scored | null>(() => {
    if (commandPrefix !== null || (mode !== 'all' && mode !== 'conversations')) return null;
    const destination = { to: '/search', search: query === '' ? {} : { q: query } };
    return {
      score: 0,
      row: {
        id: 'conversations:page',
        group: 'conversations',
        title: query === '' ? t('finder.conversations.advanced') : t('finder.conversations.advancedQuery', { query }),
        marks: [],
        detail: '',
        icon: MessagesSquare,
        destination,
        run: (open) => env.go(destination, open),
      },
    };
  }, [query, commandPrefix, mode, t, env]);

  const answered = useAnswers({
    commandPrefix,
    active: open,
    metadataReady: ready,
    mode,
    query,
    env,
    searchRow: null,
    helpForScreen: () => openHelp(),
    commandsHere: () => {
      walked.current = false;
      setWalkedTo(null);
      setText(SIGNS.commands);
    },
  });

  // Hits of what was said, as rows that open at the hit.
  const hitRows = useMemo<Scored[]>(() => {
    if (!wantsHits || preview === undefined) return [];
    const labels = labelsOfHits(preview.items, env.knownPlaces);
    return preview.items.flatMap((hit, index) => {
      const destination = destinationOfHit(hit, slugOfOrg);
      const target = targetOfHit(hit);
      const way = labelsAlongPath(wayOfHit(hit), labels, () => '')
        .filter((name) => name !== '')
        .join(' › ');
      const files = filesOfHit(hit).filter((source) => hit.kind === 'document' || markedRanges(source.name, query).length > 0);
      if (files.length > 0) return files.map((source, fileIndex): Scored => {
        const addressed = hit.kind === 'document' && source.fileId !== undefined && destination !== null
          ? { ...destination, search: { ...destination.search, file: source.fileId } } : destination;
        const fileSource = source.fileId !== undefined && hit.kind === 'document'
          ? { ...source, href: `/doc/${hit.document.id}?file=${source.fileId}` } : source;
        return { score: 1 - index / 100, row: { id: `conversations:file:${index}:${fileIndex}`, group: 'conversations',
          file: fileSource, title: source.name, marks: markedRanges(source.name, query),
          detail: hit.kind === 'document' ? [way, hit.document.title].filter(Boolean).join(' › ') : way,
          icon: MessagesSquare, adminAccess: adminAccessOf(hit), ...(addressed === null ? {} : { destination: addressed }),
          run: (where) => { if (where === 'tab' && addressed !== null) env.go(addressed, where); else setFile(fileSource); } } };
      });
      const parts = excerptOf(hit.snippet, hit.matches);
      // The marks are in UTF-16 units of the text they are drawn on.
      let at = 0;
      const marks: [number, number][] = [];
      for (const part of parts) {
        if (part.marked) marks.push([at, at + part.text.length]);
        at += part.text.length;
      }
      const row: FinderRow = {
        id: `conversations:${hit.kind}:${index}`,
        group: 'conversations',
        adminAccess: adminAccessOf(hit),
        title: hit.snippet,
        marks,
        detail: way,
        icon: MessagesSquare,
        ...(destination === null ? {} : { destination }),
        beside: target !== null,
        run: (open) => {
          if (open === 'beside' && target !== null) openResult(target, destination);
          else if (destination !== null) env.go(destination, open);
        },
      };
      return [{ row, score: 1 - index / 100 }];
    });
  }, [wantsHits, preview, slugOfOrg, env, query]);

  const answers = { ...answered.answers };
  // Content hits stay in their ordinary group; advanced search follows every group.
  if (hitRows.length > 0) answers.conversations = hitRows;
  const continuations: Partial<Record<GroupId, FinderRow>> = {};
  for (const group of answered.full) {
    const destination = { to: '/everything', search: { q: query, participating: participating ? 'true' : undefined } };
    continuations[group] = {
      id: `everything:${group}`, group, title: t('finder.moreInEverything'), marks: [],
      detail: t('finder.moreInEverythingAbout'), icon: ArrowRight, destination,
      run: (how) => env.go(destination, how),
    };
  }
  const groups = assemble({ mode, empty: query === '', answers, expanded, t, continuations });

  const rows = [...groups.flatMap((group) => group.rows), ...(searchRow === null ? [] : [searchRow.row])];
  const byId = new Map(rows.map((row) => [row.id, row]));
  const expand = (id: GroupId) => setExpanded((before) => new Set(before).add(id));
  // The continuation is a keyboard choice too; otherwise its selected id is immediately reset to row one.
  for (const group of groups) if (group.more > 0) byId.set(`more:${group.id}`, {
    id: `more:${group.id}`, group: group.id, title: t('finder.more', { count: group.more }), marks: [], detail: '', icon: ArrowRight, keeps: true, run: () => expand(group.id),
  });
  const firstId = rows.find((row) => row.disabled === undefined)?.id ?? '';
  const chosen = walkedTo !== null && byId.has(walkedTo) ? walkedTo : firstId;
  const chosenRow = byId.get(chosen);

  const change = (next: string) => {
    walked.current = false;
    setWalkedTo(null);
    setText(next);
  };

  const activate = (row: FinderRow, where: OpenHow) => {
    howRef.current = 'here';
    if (row.disabled !== undefined) return;
    if (where === 'tab' && row.destination === undefined) return;
    if (row.target?.kind === 'tag') void recordItemVisit(row.target).catch(() => undefined);
    if (row.keeps === true && where !== 'tab') {
      row.run(where);
      return;
    }
    if (activated.current) return;
    activated.current = true;
    closeFinder();
    // After the finder has given the keyboard back, so that what opens takes it and does not lose it.
    setTimeout(() => row.run(where), 0);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (WALKING.has(event.key)) walked.current = true;
    if (event.key !== 'Enter' || event.nativeEvent.isComposing) return;
    const where = howOf(event);
    if (where === 'here') return;
    // Control or Command, and Alt, with Enter are asked for here; the list takes only a plain Enter.
    event.preventDefault();
    if (chosenRow === undefined) return;
    if (where === 'beside' && chosenRow.beside !== true) return;
    activate(chosenRow, where);
  };

  const failure = answered.failure;
  const pending = answered.pending;
  const bare = query === '' && mode !== 'all';
  // The row that offers to search what was said is there whatever was typed: it is an offer, and no answer.
  const found = mode === 'all' ? rows.filter((row) => row.group !== 'conversations') : rows;
  const nothing = failure === null && found.length === 0 && !pending && !bare && query !== '' && mode !== 'conversations';
  const pageOf = card === null ? null : destinationOf(card);

  return (
    <><Palette
      open={open}
      onOpenChange={(next) => !next && closeFinder()}
      label={t(commandPrefix === null ? 'finder.title' : 'workbench.openView.choose')}
      showTitle={viewsOnly}
      inputLabel={t(commandPrefix === null ? 'finder.field' : 'workbench.openView.search')}
      value={chosen}
      onValueChange={(next) => {
        if (walked.current) setWalkedTo(next);
      }}
      onKeyDown={onKeyDown}
    >
      <PaletteInput
        value={text}
        onValueChange={change}
        placeholder={t(commandPrefix === null ? 'finder.placeholder' : 'workbench.openView.placeholder')}
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        data-autofocus
      />
      {commandPrefix === null && <ModeButtons mode={mode} onChoose={(next) => change(withMode(text, next))} />}
      {commandPrefix === null && (mode === 'all' || mode === 'people' || mode === 'conversations') && <div className="px-4 py-2" onKeyDown={(event) => {
        if (['Enter', ' ', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) event.stopPropagation();
      }}><FilterBar participation={<ParticipationFilter checked={participating} onCheckedChange={(on) => useFinder.setState({ participating: on })} />} /></div>}

      {card !== null && (
        <Card
          hit={card}
          onBack={() => setCard(null)}
          onPage={
            pageOf !== null
              ? () => {
                  closeFinder();
                  setTimeout(() => env.go(pageOf, 'here'), 0);
                }
              : null
          }
        />
      )}
      {/* The list stays while a card is shown, out of sight: the field points at it. */}
      <PaletteList label={t('finder.list')} hidden={card !== null} onPointerMove={() => (walked.current = true)}>
          {nothing && (
            <EmptyState compact className="px-4" title={t('finder.nothing', { query })}
              description={letters < SEARCH_MIN_CHARS && (mode === 'all' || mode === 'people') ? t('finder.namesMinimum') : participating ? t('finder.nothingFocused', { focus: t('adminReach.focus.only') }) : t('finder.nothingAbout')} />
          )}
          {groups.map((group) => (
            <PaletteGroup key={group.id} heading={viewsOnly ? undefined : group.heading}>
              {group.rows.map((row) => (
                <RowView key={row.id} row={row} onSelect={() => activate(row, howRef.current)} howRef={howRef} />
              ))}
              {group.more > 0 && (
                <PaletteItem
                  value={`more:${group.id}`}
                  onSelect={() => expand(group.id)}
                  className="text-text-muted"
                >
                  {t('finder.more', { count: group.more })}
                </PaletteItem>
              )}
            </PaletteGroup>
          ))}
          {answered.partial && !pending && <p role="status" className="px-4 py-2 text-sm text-text-muted">{t('finder.namesPartial')}</p>}
          {pending && found.length === 0 && <SkeletonRows rows={3} />}
          {bare && !pending && groups.length === 0 && <PaletteEmpty>{t(`finder.bare.${mode}`)}</PaletteEmpty>}
          {failure !== null && !pending && (
            <ErrorState compact error={failure} onRetry={() => void queryClient.refetchQueries({ queryKey: ['names'], type: 'active' })} />
          )}
          {searchRow !== null && <PaletteGroup><RowView row={searchRow.row} onSelect={() => activate(searchRow.row, howRef.current)} howRef={howRef} /></PaletteGroup>}
      </PaletteList>

      <Legend row={card === null ? chosenRow : undefined} />
      <p role="status" className="sr-only">
        {pending ? '' : nothing ? t('finder.nothing', { query }) : rows.length === 0 ? '' : t('finder.results', { count: rows.length })}
      </p>
    </Palette>
      {file !== null && <FileOverlay source={file} onClose={() => setFile(null)} />}
      {tagChoice?.tag !== undefined && tagChoice.tag !== null && tagChoice.path[0] !== undefined && <TagProjects tag={tagChoice.tag} rootOrgId={tagChoice.path[0].id} onClose={() => setTagChoice(null)} onChoose={(destination) => { setTagChoice(null); env.go(destination, 'here'); }} />}
    </>
  );
}

/** The buttons under the field that write or take away the sign of a mode. */
function ModeButtons({ mode, onChoose }: { mode: Mode; onChoose: (mode: Mode) => void }) {
  const { t } = useTranslation();
  return (
    <div role="group" aria-label={t('finder.modes.label')} className="flex flex-wrap items-center gap-1 border-b border-border px-3 py-2">
      {MODES.map((candidate) => (
        <button
          key={candidate}
          type="button"
          aria-pressed={mode === candidate}
          onClick={() => onChoose(candidate)}
          className={cx(
            'inline-flex h-(--control-sm) items-center gap-1 rounded-md px-2 text-sm transition-colors',
            mode === candidate ? 'bg-accent-subtle text-text' : 'text-text-muted hover:bg-bg-muted hover:text-text',
          )}
        >
          {candidate !== 'all' && <span aria-hidden className="font-mono">{SIGNS[candidate]}</span>}
          {t(`finder.modes.${candidate}`)}
        </button>
      ))}
    </div>
  );
}

/** The icon of a place, in the colour it was given where the tree that holds it is known. */
function PlaceGlyph({ kind, id, color, own, display }: { kind: NodeIconKind; id: string; color?: number | null; own?: string | null; display?: ThreadDisplay }) {
  const palette = useNodeColor(id);
  const custom = useNodeOwnColor(id);
  if (kind === 'thread') return <ThreadStateIcon display={display} />;
  return <NodeIcon kind={kind} color={color === undefined ? palette : color} own={own === undefined ? custom : own} />;
}

/** One row of the list. */
function RowView({ row, onSelect, howRef }: { row: FinderRow; onSelect: () => void; howRef: { current: OpenHow } }) {
  if (row.id.startsWith('commands:view.open.') || row.id.startsWith('commands:view.arrangement.')) {
    return <WorkbenchChoiceRow row={row} onSelect={onSelect} onClick={(event) => (howRef.current = howOf(event))} />;
  }
  const Icon = row.icon;
  return (
    <PaletteItem value={row.id} disabled={row.disabled !== undefined} onSelect={onSelect} className={cx(row.archived === true && 'text-text-muted')}>
      {/* A click is read here, where its modifiers are known; the select of the row that follows it does not know them. */}
      <span className="flex min-w-0 flex-1 items-center gap-3" onClick={(event) => (howRef.current = howOf(event))}>
        {row.file !== undefined ? null : row.place === undefined ? <Icon aria-hidden className="text-text-muted" /> : <PlaceGlyph {...row.place} />}
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="flex min-w-0 items-center gap-2">
            {row.file === undefined ? <span className="truncate"><Marked text={row.title} ranges={row.marks} /></span> :
              <FileChip source={row.file} bare label={<Marked text={row.title} ranges={row.marks} />} ariaLabel={row.title} presentation={{ place: 'compact' }} onOpen={(event) => { howRef.current = howOf(event); onSelect(); }} />}
            <AsAdminMark of={row.adminAccess?.place} responsive />
          </span>{' '}
          {(row.detail !== '' || row.state !== undefined || row.lastActivity !== undefined) && <span className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted"><span className="min-w-0 wrap-anywhere">{row.detail}</span>{row.state !== undefined && <span>{row.state}</span>}{row.lastActivity !== undefined && <RelativeTime at={row.lastActivity} />}</span>}
        </span>
        {row.starred === true && <Star aria-hidden className="text-warning" />}
        {row.note !== undefined && <span className="shrink-0 text-xs text-text-muted">{row.note}</span>}
        {row.shortcut !== undefined && <Kbd shortcut={row.shortcut} />}
      </span>
    </PaletteItem>
  );
}

/** What the keys do now, for the row that is chosen. */
function Legend({ row }: { row: FinderRow | undefined }): ReactNode {
  const { t } = useTranslation();
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-4 py-2 text-xs text-text-muted">
      <span className="flex items-center gap-1">
        <Kbd shortcut="Enter" /> {row?.destination === undefined && row?.keeps !== true && row !== undefined && row.file === undefined ? t('finder.legend.run') : t('finder.legend.open')}
      </span>
      {row?.destination !== undefined && (
        <span className="flex items-center gap-1">
          <Kbd shortcut="Mod+Enter" /> {t('finder.legend.tab')}
        </span>
      )}
      {row?.beside === true && (
        <span className="flex items-center gap-1">
          <Kbd shortcut="Alt+Enter" /> {t(row.besideAct === 'chat' ? 'finder.legend.chat' : 'finder.legend.beside')}
        </span>
      )}
      <span className="flex items-center gap-1">
        <Kbd shortcut="Escape" /> {t('finder.legend.close')}
      </span>
      {row?.target !== undefined && <SavedStar key={`${row.target.kind}:${row.target.id}`} target={row.target} name={row.title} starred={row.starred === true} />}
    </div>
  );
}

/** One footer action works for every real name, without nesting a button in a listbox option. */
function SavedStar({ target, name, starred }: { target: NonNullable<FinderRow['target']>; name: string; starred: boolean }) {
  const { t } = useTranslation();
  const save = useMutation({ mutationFn: () => starred ? unstarItem(target) : starItem(target) });
  return <span className="flex min-w-0 items-center gap-2">
    <Button size="sm" variant="ghost" icon={<Star aria-hidden />} pending={save.isPending} onClick={() => save.mutate()}>{t(starred ? 'finder.unstar' : 'finder.star', { name })}</Button>
    {save.error !== null && <ErrorAlert error={save.error} />}
  </span>;
}

