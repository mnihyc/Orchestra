/** Export… on an org, a project, a folder or a topic: one archive of what its person may read there, chosen in a tree, planned as the choice changes, and written by a job.
 *
 * The tree has a tick on every row the person may read, all ticked as it opens; a place that is ticked takes
 * what is in it. Under it the kinds, the form, the visible content/date options and the plan: how much the archive would
 * hold against what the place allows, asked again as anything changes. Past a limit the button stays off and the
 * line says by how much. Export starts the job and closes the dialog; the status bar shows the job, and the
 * person is told when it ends.
 */
import { keepPreviousData, useInfiniteQuery, useMutation, useQueries, useQuery } from '@tanstack/react-query';
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { askForExport, documentsUnderQuery, exportKeys, planExport, readOwnDocuments, topicThreadsQuery } from '@/api/exports';
import { nodeQuery } from '@/api/nodes';
import { Hint } from '@/features/help/Hint';
import type { DocumentRef, ExportPlan, ExportRequest } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { useOrgTreeFeed } from '@/realtime/live';
import { useOrgTree } from '@/realtime/stores/tree';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import type { TreeRow } from '@/ui/Tree';

import { type Choices, OPENING_CHOICES, requestOf, spanFault, wantsNothing } from './choices';
import { Group } from './Group';
import { watchExport } from './jobs';
import { mayExportThread } from './pending';
import { type PlanLineProps, PlanLine } from './PlanLine';
import { PLAN_PAUSE_MS, readPlan } from './plan';
import { PlaceOptions } from './PlaceOptions';
import { PlaceTree } from './PlaceTree';
import { documentsBelow, layoutRows, type RowItem, type RowMemory } from './rows';
import { readTicks, type Scope, scopeOf, tallies, tickRow, type Ticks, wantedOf } from './selection';
import type { FileDialogProps } from './ThreadFile';
import { mentionsWorkspace, WorkspaceNote } from './WorkspaceNote';

/** How many pages of the documents under a place are read at once, for the words on the rows: 200 documents a page, so two thousand. */
const SCAN_PAGES_MAX = 10;

const NO_DOCUMENTS: { items: readonly DocumentRef[]; complete: boolean } = { items: [], complete: true };

export function PlaceExport({ target, open, onClose }: FileDialogProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const actKey = useActKey();

  // ---- the place and its tree
  const node = useQuery(nodeQuery(target.id));
  const orgId = node.data?.org_id ?? null;
  useOrgTreeFeed(orgId);
  const tree = useOrgTree(orgId);
  const ready = node.isSuccess && tree.loaded && tree.nodes.has(target.id);

  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set([target.id]));
  const [ticks, setTicks] = useState<Ticks>(() => new Map());
  const [choices, setChoices] = useState<Choices>(OPENING_CHOICES);

  // ---- the documents: everything under the place, a page at a time, for the rows and their words
  const scan = useInfiniteQuery({ ...documentsUnderQuery(target.id), enabled: ready });
  const scanPages = scan.data?.pages.length ?? 0;
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = scan;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && scanPages < SCAN_PAGES_MAX) void fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage, scanPages]);
  const scanComplete = scan.isSuccess && !scan.hasNextPage;
  const scanned = useMemo(() => {
    const byPlace = new Map<string, DocumentRef[]>();
    for (const page of scan.data?.pages ?? []) {
      for (const document of page.items) {
        const list = byPlace.get(document.node.id);
        if (list === undefined) byPlace.set(document.node.id, [document]);
        else list.push(document);
      }
    }
    return byPlace;
  }, [scan.data]);
  // A place partly ticked is named by its own documents one by one, so they are read whole when the scan did not reach them all.
  const [own, setOwn] = useState<ReadonlyMap<string, readonly DocumentRef[] | 'too-many'>>(() => new Map());
  const asking = useRef(new Set<string>());

  // ---- what the caller may not take away, from what the places and their threads say they hold
  const visible = useMemo(() => [...expanded].filter((id) => tree.nodes.get(id)?.kind !== 'thread' && tree.nodes.has(id)), [expanded, tree.nodes]);
  const places = useQueries({ queries: visible.map((id) => ({ ...nodeQuery(id), enabled: ready })) });
  const topics = useQueries({ queries: visible.filter((id) => tree.nodes.get(id)?.kind === 'topic').map((id) => ({ ...topicThreadsQuery(id), enabled: ready })) });
  // What the caller reads and may not take away, as a key: the queries hand out a new array at every drawing, and the set is made again only when what is in it changes.
  const greyedList: string[] = [];
  for (const found of topics) for (const thread of found.data ?? []) if (!mayExportThread(thread)) greyedList.push(thread.id);
  visible.forEach((id, at) => {
    const place = places[at]?.data;
    if (place === undefined || holds(place.access, 'doc.export')) return;
    // A document that follows its place's rows cannot be taken away where the place's row lacks it; one with a page of its own is the server's to say.
    for (const document of scanned.get(id) ?? []) if (document.page === 'follows') greyedList.push(document.id);
  });
  const greyedKey = greyedList.toSorted().join('|');
  const greyed = useMemo(() => new Set(greyedKey === '' ? [] : greyedKey.split('|')), [greyedKey]);

  // ---- the scope the ticks are laid on
  const documentsOf = useCallback(
    (place: string): { items: readonly DocumentRef[]; complete: boolean } => {
      const kind = tree.nodes.get(place)?.kind;
      if (kind === undefined || kind === 'thread') return NO_DOCUMENTS;
      const whole = own.get(place);
      if (whole !== undefined && whole !== 'too-many') return { items: whole, complete: true };
      return { items: scanned.get(place) ?? [], complete: scanComplete };
    },
    [tree.nodes, own, scanned, scanComplete],
  );
  const scope: Scope = useMemo(() => scopeOf(tree.nodes, target.id, locale, documentsOf, greyed), [tree.nodes, target.id, locale, documentsOf, greyed]);
  const reading = useMemo(() => readTicks(scope, ticks), [scope, ticks]);
  const counts = useMemo(() => tallies(scope), [scope]);
  const below = useMemo(() => documentsBelow(scope, scanned), [scope, scanned]);

  // ---- what the ticks and the choices come to
  const wanted = useMemo(() => wantedOf(scope, ticks, { chats: choices.chats, documents: choices.documents }), [scope, ticks, choices.chats, choices.documents]);
  const needed = wanted.needDocuments.join(',');
  useEffect(() => {
    for (const place of needed === '' ? [] : needed.split(',')) {
      if (asking.current.has(place)) continue;
      asking.current.add(place);
      // A list that cannot be read cannot be named either: the person is told to tick whole places, which needs none.
      void readOwnDocuments(place).then(
        (found) => setOwn((held) => new Map(held).set(place, found ?? 'too-many')),
        () => setOwn((held) => new Map(held).set(place, 'too-many')),
      );
    }
  }, [needed]);
  const tooManyDocuments = wanted.needDocuments.some((place) => own.get(place) === 'too-many');
  const waitingForDocuments = wanted.needDocuments.length > 0 && !tooManyDocuments;

  let idle: PlanLineProps['idle'] = null;
  if (wantsNothing(choices)) idle = 'no-kind';
  else if (spanFault(choices) !== null) idle = 'span';
  else if (wanted.tooMany || tooManyDocuments) idle = 'too-many-ticks';
  else if (wanted.wanted.type === 'nothing') idle = 'nothing-ticked';

  const request: ExportRequest | null = useMemo(
    () => (idle !== null || waitingForDocuments || wanted.wanted.type === 'nothing' ? null : requestOf(wanted.wanted, choices)),
    [idle, waitingForDocuments, wanted.wanted, choices],
  );
  const requestKey = request === null ? null : JSON.stringify(request);
  const settledKey = useDebounced(requestKey, PLAN_PAUSE_MS);
  const asked: ExportRequest | null = useMemo(() => (settledKey === null ? null : (JSON.parse(settledKey) as ExportRequest)), [settledKey]);

  const plan = useQuery({
    queryKey: asked === null ? ['exports', 'plan', target.id, null] : exportKeys.plan(target.id, asked),
    queryFn: async ({ signal }): Promise<ExportPlan> => {
      if (asked === null) throw new Error('no request to plan');
      return planExport(target.id, asked, signal);
    },
    enabled: ready && asked !== null,
    placeholderData: keepPreviousData,
    // A plan is a reading of what is there now: asked for again each time, never taken from a minute ago.
    staleTime: 0,
    gcTime: 30_000,
    retry: false,
    // The dialog is in front: coming back to the window does not make the figures blink and the button go off.
    refetchOnWindowFocus: false,
  });
  const readingOfPlan = useMemo(() => (plan.data === undefined ? null : readPlan(plan.data)), [plan.data]);
  const counting = request !== null && (requestKey !== settledKey || plan.isFetching || waitingForDocuments);
  const mayAsk = request !== null && !counting && !plan.isError && readingOfPlan?.state === 'ok';

  // ---- asking
  const create = useMutation({
    mutationFn: async () => {
      if (asked === null) throw new Error('no request to make');
      return askForExport(target.id, asked, actKey(asked));
    },
    onSuccess: (job) => {
      watchExport(job, target.name);
      toast({ tone: 'info', title: t('export.job.started', { place: target.name }) });
      onClose();
    },
    onError: (error) => {
      // What the plan said no longer holds: the place changed meanwhile.
      if (error instanceof ApiError && error.code.startsWith('export.')) void plan.refetch();
    },
  });

  // ---- the acts on the tree
  const [memory] = useState<RowMemory>(() => new Map());
  const rows = useMemo(
    () => layoutRows({ scope, expanded, reading, tallies: counts, documentsBelow: below, documentsComplete: scanComplete, leftOutIn: readingOfPlan?.notExportableIn ?? new Map(), memory }),
    [scope, expanded, reading, counts, below, scanComplete, readingOfPlan, memory],
  );
  const onTick = useCallback((row: TreeRow<RowItem>, on: boolean) => setTicks((held) => tickRow(scope, held, row.id, on)), [scope]);
  const onExpandedChange = useCallback(
    (id: string, on: boolean) =>
      setExpanded((held) => {
        const next = new Set(held);
        if (on) next.add(id);
        else next.delete(id);
        return next;
      }),
    [],
  );
  const change = useCallback((patch: Partial<Choices>) => setChoices((held) => ({ ...held, ...patch })), []);

  const rateLimited = create.error instanceof ApiError && create.error.is('rate_limited');

  let body: ReactNode;
  if (node.isError) body = <ErrorState compact error={node.error} retrying={node.isRefetching} onRetry={() => void node.refetch()} />;
  else if (!ready) body = <SkeletonRows rows={5} />;
  else body = <PlaceTree rows={rows} onExpandedChange={onExpandedChange} onTick={onTick} className="py-1" />;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !create.isPending && onClose()}
      title={t('export.place.title', { name: target.name })}
      description={t('export.place.lead')}
      size="lg"
      dismissible={!create.isPending}
      footer={
        <>
          <Button variant="secondary" disabled={create.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" aria-describedby="export-plan" pending={create.isPending} blocked={!mayAsk} onClick={() => create.mutate()}>
            {t('export.place.do')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Group label={t('export.what.label')} help={<Hint id="export-what" />} lead="box">
          <div className="max-h-[16rem] min-h-[6rem] overflow-y-auto rounded-lg border border-border bg-surface">{body}</div>
          {ready && !scanComplete && scanPages >= SCAN_PAGES_MAX && <p className="mt-1 text-xs text-text-muted">{t('export.what.documentsCut')}</p>}
        </Group>
        <PlaceOptions choices={choices} onChange={change} />
        {ready && mentionsWorkspace(node.data) && <WorkspaceNote target={target} onClose={onClose} />}
        <div id="export-plan" className="border-t border-border pt-3">
          <PlanLine reading={readingOfPlan} counting={counting} error={plan.isError ? plan.error : null} onRetry={() => void plan.refetch()} idle={idle} />
        </div>
        {rateLimited ? <InlineAlert tone="warning">{t('errors.export.at_once')}</InlineAlert> : create.isError && <ErrorAlert error={create.error} />}
      </div>
    </Dialog>
  );
}
