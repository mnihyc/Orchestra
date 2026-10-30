/** The three lists of approvals, page by page, and the cards each draws: what waits for the person with its decision controls, what they asked for with its answer, and what was decided.
 *
 * A request that leaves the waiting list while the person looks at it (they decided it, or somebody else
 * did first) does not vanish: it turns into its decided line in the place it stood, and says who answered,
 * until the person leaves the list. Requests one person sent together are one card with a row for each.
 */
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { ShieldCheck } from 'lucide-react';
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { asPortApproval, PortApprovalCard } from '@/features/ports/Request';
import { approvalQuery, approvalsQuery, type ApprovalsFilter, cancelApproval } from '@/api/approvals';
import { meQuery } from '@/api/auth';
import { AccessCard, type AccessApproval } from '@/features/approvals/Access';
import { CapabilityCard } from '@/features/approvals/Capability';
import { SourceCard, SourceBrief } from '@/features/approvals/Source';
import { approvalTitle, ApprovalHeadings, CardFrame, DecidedCard, kindWord } from '@/features/approvals/Card';
import { useDecision } from '@/features/approvals/Decision';
import { DecisionError } from '@/features/approvals/DecisionError';
import { AnswersForm, decisionOfOption, ElicitationForm, type HarnessApproval, OptionButtons, planDecision, RequestBody } from '@/features/approvals/Harness';
import { PlanAnswer } from '@/features/approvals/PlanAnswer';
import type { Approval } from '@/generated/api-types';
import { permissionLabel } from '@/lib/permissions';
import { useSocketStatus, useUserFeed } from '@/realtime/live';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

/** Whether answering is possible now: decisions depend on current state, so they wait for the live connection. */
export function useLive(): boolean {
  return useSocketStatus().phase === 'ready';
}

/** An Inbox link opens this exact request, even when it lies beyond every loaded list page. */
export function ApprovalDetail({ id }: { id: string }) {
  const { t } = useTranslation();
  useUserFeed(false);
  useScreenHeading({ title: t('approvals.detailTitle'), icon: ShieldCheck,
    trail: [{ label: t('screens.approvals.title'), to: '/approvals' }] });
  const query = useQuery(approvalQuery(id));
  const [decided, setDecided] = useState<Approval | null>(null);
  const detail = useRef<HTMLDivElement>(null);
  useEffect(() => { if (decided !== null) detail.current?.querySelector<HTMLButtonElement>('button')?.focus(); }, [decided]);
  const approval = decided ?? query.data;
  return <Page>
    <PageHeader title={t('approvals.detailTitle')} mark={<PlaceMark><ShieldCheck aria-hidden /></PlaceMark>} />
    <ApprovalHeadings level={2}>
      <div ref={detail} data-focused-approval={id}>
        {approval === undefined ? query.isError ? <ErrorState error={query.error} onRetry={() => void query.refetch()} /> : <Loading />
          : approval.state !== 'pending' ? <DecidedCard approval={approval}><RequestBrief approval={approval} /></DecidedCard>
            : <Waiting items={[approval]} settled={new Map<string, Approval>()} remember={setDecided} more={null} />}
      </div>
    </ApprovalHeadings>
  </Page>;
}

/** What a list of approvals is of, and what it says when it is empty. */
export interface ListShape {
  filter: ApprovalsFilter;
  empty: { title: string; description: string };
}

function Loading() {
  return (
    <SkeletonGroup className="flex flex-col gap-3">
      {Array.from({ length: 2 }, (_, index) => (
        <div key={index} className="flex flex-col gap-3 rounded-lg border border-border bg-surface px-4 py-4">
          <Skeleton className="h-4 w-1/4" />
          <Skeleton className="h-5 w-1/2" />
          <Skeleton className="h-16 w-full" />
        </div>
      ))}
    </SkeletonGroup>
  );
}

/** One list of approvals as the server pages it. */
export function useApprovalPages(filter: ApprovalsFilter) {
  const query = useInfiniteQuery(approvalsQuery(filter));
  const items = useMemo(() => {
    const seen = new Set<string>();
    const all: Approval[] = [];
    for (const page of query.data?.pages ?? []) {
      for (const approval of page.items) {
        if (seen.has(approval.id)) continue;
        seen.add(approval.id);
        all.push(approval);
      }
    }
    return all;
  }, [query.data]);
  return { query, items };
}

/**
 * The shell of a list: while it loads, when it failed, when it is empty, and the page after. `kept` is how
 * many cards stand in the list that the server no longer lists (what was decided meanwhile): while there are
 * any the list is not empty, and what draws them stays where it is.
 */
function ListFrame({
  list,
  empty,
  kept = 0,
  children,
}: {
  list: ReturnType<typeof useApprovalPages>;
  empty: ListShape['empty'];
  kept?: number;
  children: (more: ReactNode) => ReactNode;
}) {
  const { query, items } = list;
  const more = <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />;
  if (query.isPending) return <Loading />;
  if (query.isError && query.data === undefined) return <ErrorState error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  return (
    <div className="flex flex-col gap-3">
      {query.isError && <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />}
      {items.length === 0 && kept === 0 ? <EmptyState icon={<ShieldCheck aria-hidden />} title={empty.title} description={empty.description} /> : children(more)}
    </div>
  );
}

/** Requests one person sent together stand as one card: the first of a batch takes the others in. */
export function groupBatches(items: readonly Approval[]): Approval[][] {
  const groups: Approval[][] = [];
  const at = new Map<string, Approval[]>();
  for (const item of items) {
    if (item.kind === 'access' && item.batch_id !== null) {
      const group = at.get(item.batch_id);
      if (group !== undefined) {
        group.push(item);
        continue;
      }
      const made = [item];
      at.set(item.batch_id, made);
      groups.push(made);
    } else {
      groups.push([item]);
    }
  }
  return groups;
}

// ---------------------------------------------------------------- what waits for the person

/** The request of a capability or an access card in brief, for a decided line that opens. */
function RequestBrief({ approval }: { approval: Approval }) {
  const { t } = useTranslation();
  const port = asPortApproval(approval);
  if (port !== null) return <div className="flex flex-wrap gap-x-3 gap-y-1"><span>{port.request.environment.name}</span><span>{t('ports.portNumber',{port:port.request.request.port})}</span><span>{port.request.plan.audience.sentence}</span></div>;
  if(approval.kind==='plan'||approval.kind==='input')return <SourceBrief approval={approval}/>;
  if (approval.kind === 'harness') return <RequestBody request={approval.request} />;
  if (approval.kind === 'capability') {
    return (
      <>
        <p className="flex flex-wrap gap-x-3 gap-y-1"><span>{approvalTitle(approval,t)}</span><span>{approval.request.environment.name}</span><span>{approval.request.host.name}</span></p>
        {approval.request.reason !== '' && <p>{approval.request.reason}</p>}
      </>
    );
  }
  // A port's request was shown above; what is left is a request for access.
  if (approval.kind !== 'access') return null;
  return (
    <>
      <p>{approval.request.permissions.map((permission) => permissionLabel(permission)).join(', ')}</p>
      {approval.request.reason !== '' && <p>{approval.request.reason}</p>}
      <p className="sr-only">{kindWord(approval.kind, t)}</p>
    </>
  );
}

function HarnessCard({ approval, live, onSettled }: { approval: HarnessApproval; live: boolean; onSettled: (approval: Approval) => void }) {
  const { t } = useTranslation();
  const decision = useDecision(approval.id, { onDecided: onSettled, onLost: onSettled });
  const request = approval.request;
  const may = approval.can_decide && live;
  const busy = decision.pending;
  return (
    <CardFrame approval={approval}>
      <RequestBody request={request} />
      {request.request_kind === 'user_input' ? (
        <AnswersForm questions={request.detail.questions} disabled={!may || busy} pending={busy} onSend={(answers) => decision.decide({ answers }, t('approvals.sendAnswers'))} />
      ) : request.request_kind === 'mcp_elicitation' ? (
        <ElicitationForm
          schema={request.detail.schema}
          message={request.detail.message}
          disabled={!may || busy}
          pending={busy}
          onSend={(action, content) => decision.decide({ action, ...(content === undefined ? {} : { content }) }, t('approvals.accept'))}
        />
      ) : request.request_kind === 'plan' ? (
        <PlanAnswer disabled={!may} pending={decision.pressed === t('plan.proceed') ? 'proceed' : decision.pressed === t('plan.keepPlanning') ? 'keep' : null}
          onProceed={() => decision.decide(planDecision(true), t('plan.proceed'))}
          onKeepPlanning={(words) => decision.decide(planDecision(false, words), t('plan.keepPlanning'))} />
      ) : (
        <OptionButtons target={request.request_kind === 'command' ? request.detail.command : approval.title} options={request.options} disabled={!may || busy} pressed={decision.pressed} onChoose={(option) => decision.decide(decisionOfOption(option), option.label)} />
      )}
      {!approval.can_decide && <p className="text-sm text-text-muted">{t('approvals.cannotAnswer')}</p>}
      {approval.can_decide && !live && <p className="text-sm text-text-muted">{t('approvals.waitForLine')}</p>}
      {decision.error !== null && <DecisionError error={decision.error} />}
    </CardFrame>
  );
}

/**
 * What leaves the waiting list while it is on screen: remembered here as the approval it became, so
 * that it can be drawn as a decided line in its place. `remember` is told of what the person decided.
 */
function useSettled(items: readonly Approval[]) {
  const queryClient = useQueryClient();
  const [settled, setSettled] = useState<ReadonlyMap<string, Approval>>(new Map());
  const known = useRef(new Set<string>());
  const seen = useRef(new Map<string, Approval>());
  const remember = (approval: Approval) => {
    known.current.add(approval.id);
    setSettled((held) => new Map(held).set(approval.id, approval));
  };
  useEffect(() => {
    const present = new Set(items.map((item) => item.id));
    for (const id of [...seen.current.keys()]) {
      if (present.has(id) || known.current.has(id)) continue;
      seen.current.delete(id);
      known.current.add(id);
      // It left the list: somebody decided it, or it was taken back. Reading it says which.
      queryClient.query({ ...approvalQuery(id), staleTime: 0 }).then(
        (approval) => {
          if (approval.state !== 'pending') setSettled((held) => new Map(held).set(id, approval));
        },
        () => undefined,
      );
    }
    for (const item of items) if (!known.current.has(item.id)) seen.current.set(item.id, item);
  }, [items, queryClient]);
  return { settled, remember };
}

/** What the person has to decide. */
export function WaitingList({ shape }: { shape: ListShape }) {
  const list = useApprovalPages(shape.filter);
  // Held here, above the check for an empty list: what was the last card to be decided must not take its line with it.
  const { settled, remember } = useSettled(list.items);
  return (
    <ListFrame list={list} empty={shape.empty} kept={settled.size}>
      {(more) => <Waiting items={list.items} settled={settled} remember={remember} more={more} />}
    </ListFrame>
  );
}

function Waiting({ items, settled, remember, more }: { items: Approval[]; settled: ReadonlyMap<string, Approval>; remember: (approval: Approval) => void; more: ReactNode }) {
  const live = useLive();
  const me = useQuery(meQuery).data;
  // What was decided meanwhile stays where it stood: the cards in the order they were made, newest first.
  const cards = useMemo(() => {
    const present = new Set(items.map((item) => item.id));
    const gone = [...settled.values()].filter((approval) => !present.has(approval.id));
    return [...items, ...gone].sort((a, b) => (a.created_at < b.created_at ? 1 : a.created_at > b.created_at ? -1 : 0));
  }, [items, settled]);
  return (
    <ul className="flex flex-col gap-3">
      {groupBatches(cards).map((group) => {
        const first = group[0];
        if (first === undefined) return null;
        if (first.kind === 'access') {
          return (
            <li key={first.batch_id ?? first.id}>
              <AccessCard items={group.filter((each): each is AccessApproval => each.kind === 'access')} settled={settled} live={live} onSettled={remember} />
            </li>
          );
        }
        const port = asPortApproval(first);
        if (port !== null) return <li key={port.id}><PortApprovalCard approval={port} live={live} onSettled={remember} /></li>;
        const done = settled.get(first.id);
        return (
          <li key={first.id}>
            {done !== undefined && done.state !== 'pending' ? (
              <DecidedCard approval={done} meanwhile={done.decided_by !== null && done.decided_by.id !== me?.id}>
                <RequestBrief approval={done} />
              </DecidedCard>
            ) : first.kind === 'plan' || first.kind === 'input' ? <SourceCard approval={first} live={live} onSettled={remember}/> : first.kind === 'harness' ? (
              <HarnessCard approval={first} live={live} onSettled={remember} />
            ) : first.kind === 'capability' ? (
              <CapabilityCard approval={first} live={live} onSettled={remember} />
            ) : null}
          </li>
        );
      })}
      {more}
    </ul>
  );
}

// ---------------------------------------------------------------- what the person asked for

/** A request of the person's own that still waits: what it asks, and the way to take it back. */
function RequestedCard({ approval }: { approval: Approval }) {
  const { t } = useTranslation();
  const [error, setError] = useState<unknown>(null);
  const [pending, setPending] = useState(false);
  const takeBack = () => {
    setPending(true);
    setError(null);
    cancelApproval(approval.id).then(
      () => setPending(false),
      (failure: unknown) => {
        setError(failure);
        setPending(false);
      },
    );
  };
  return (
    <CardFrame approval={approval} quiet>
      <div className="flex flex-col gap-2 text-sm text-text-muted">
        <RequestBrief approval={approval} />
      </div>
      {approval.kind === 'access' && (
        <div>
          <Button size="sm" variant="secondary" pending={pending} onClick={takeBack}>
            {t('approvals.takeBack')}
          </Button>
        </div>
      )}
      {error !== null && <ErrorAlert error={error} />}
    </CardFrame>
  );
}

export function RequestedWaitingList({ shape }: { shape: ListShape }) {
  const list = useApprovalPages(shape.filter);
  return (
    <ListFrame list={list} empty={shape.empty}>
      {(more) => (
        <ul className="flex flex-col gap-3">
          {list.items.map((approval) => (
            <li key={approval.id}>
              <RequestedCard approval={approval} />
            </li>
          ))}
          {more}
        </ul>
      )}
    </ListFrame>
  );
}

// ---------------------------------------------------------------- what was decided

/** Decided approvals, one line each, which open to the request. */
export function DecidedList({ shape }: { shape: ListShape }) {
  const list = useApprovalPages(shape.filter);
  return (
    <ListFrame list={list} empty={shape.empty}>
      {(more) => (
        <ul className="flex flex-col gap-2">
          {list.items.map((approval) => (
            <li key={approval.id}>
              <DecidedCard approval={approval}>
                <RequestBrief approval={approval} />
              </DecidedCard>
            </li>
          ))}
          {more}
        </ul>
      )}
    </ListFrame>
  );
}
