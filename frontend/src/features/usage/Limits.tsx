/** Limits card: caps on tokens, on credits and on turns at once with their meters, who is waiting, how firmly each holds, how often each was passed, changes in place.
 *
 * Everything that bounds work at a place, a provider or for a person, one row for each cap in the same
 * words wherever it is drawn: whom it covers, of which models, where it was set and by whom. A cap on
 * tokens or credits has a meter of what is used against the amount, with what turns still running have
 * reported hatched, and says when it starts again; a cap on turns at once says how many run and how many
 * wait, and what waits is listed behind the count in the order it will run. A cap that was set above is
 * shown as such and read only here, with where to change it. Whoever may change a cap changes its amount
 * in the row, switches it off, changes what it covers or removes it; whoever may add one has Add limit.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Ellipsis, Pencil, Plus, Trash2 } from 'lucide-react';
import { memo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { capsQuery, deleteCap, myLimitsQuery, placeLimitsQuery, updateCap, waitingQuery } from '@/api/usage';
import type { CapStatus, Metering, UsageCap } from '@/generated/api-types';
import { openCap } from '@/features/usage/AddCap';
import { amountText } from '@/features/usage/capForm';
import { decimal, formatCredits } from '@/features/usage/figures';
import { allowanceWords, capSentence, capTitle, startsAgain, usedWords } from '@/features/usage/words';
import { useFormatContext } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';
import { boxClasses,Section } from '@/shell/Page';
import { ForecastLine } from './ForecastLine';
import { CapTitle,CapSetter,CapShare } from './CapPeople';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { fullness } from '@/ui/Meter';
import { Popover } from '@/ui/Popover';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';
import { UserChip } from '@/ui/UserChip';

/** What the card is the card of. */
export type LimitsOf =
  /** Everything that bounds work at a place, whoever set it. */
  | { kind: 'node'; id: string; name: string; /** Whether the reader may add a cap there. */ manage: boolean; /** Whether the reader reads reports there, and so the caps defined there with those that are switched off. */ reports?: boolean }
  /** The caps defined on a provider. */
  | { kind: 'provider'; id: string; name: string; manage: boolean }
  /** The reader's own share of every cap counted per person that covers them. */
  | { kind: 'mine' };

export interface LimitsProps {
  of: LimitsOf;
  /** How the thread the reader looks from is metered, for how firmly each cap holds there; nothing outside a thread. */
  metering?: Metering | undefined;
  /** Opens the rates of the place, for whoever may set them. */
  onRates?: (() => void) | undefined;
  /** For a narrow place: the card draws no frame of its own. */
  bare?: boolean;
  className?: string;
}

/** The bar of a cap: what is used, and the part that turns still running have reported, hatched. */
export function CapMeter({ status, label }: { status: CapStatus; label: string }) {
  const amount = decimal(status.cap.amount);
  if (status.cap.amount === null || amount <= 0) {
    // An amount of nothing is full whatever is used; no limit has no bar.
    return status.cap.amount === null ? null : <span role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={100} className="block h-2 w-full rounded-full bg-danger" />;
  }
  const used = Math.min(100, (decimal(status.used) / amount) * 100);
  const running = Math.min(100 - used, (decimal(status.in_progress) / amount) * 100);
  const tone = fullness(used + running);
  const color = tone === 'danger' ? 'var(--danger)' : tone === 'warning' ? 'var(--warning)' : 'var(--accent)';
  return (
    <span role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(used)} className="flex h-2 w-full overflow-hidden rounded-full bg-bg-muted">
      <span style={{ width: `${used}%`, background: color }} />
      {running > 0 && <span style={{ width: `${running}%`, background: `repeating-linear-gradient(45deg, ${color} 0 3px, transparent 3px 6px)` }} />}
    </span>
  );
}

/** What waits for room in a cap on turns at once, in the order it will run. */
function Waiting({ capId }: { capId: string }) {
  const { t } = useTranslation();
  const query = useQuery(waitingQuery(capId));
  if (query.isPending) return <SkeletonRows rows={2} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data.items.length === 0) return <EmptyState compact title={t('caps.row.waitingNone')} />;
  return (
    <ol aria-label={t('caps.row.waitingList')} className="flex flex-col gap-1 text-sm">
      {query.data.items.map((entry, index) => (
        <li key={`${entry.since}-${index}`} className="flex flex-wrap items-center gap-x-2 text-text">
          <span className="w-5 text-text-subtle tabular-nums">{index + 1}</span>
          <span>{entry.thread?.name ?? (entry.mine ? t('caps.row.waitingYours') : t('caps.row.waitingPlace'))}</span>
          {entry.author !== null && <UserChip user={entry.author} size={24} />}
          <span className="text-text-muted">
            <RelativeTime at={entry.since} />
          </span>
        </li>
      ))}
    </ol>
  );
}

/** The amount of a cap changed in its row: a field with Save and Cancel, and what the API says when it will not. */
function AmountEditor({ cap, onDone }: { cap: UsageCap; onDone: () => void }) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState(cap.amount ?? '');
  const parsed = amountText(typed, cap.meter);
  const save = useMutation({
    mutationFn: () => updateCap(cap.id, { expected_revision: cap.revision, amount: parsed }),
    onSuccess: () => {
      toast({ tone: 'success', title: t('caps.form.changed') });
      onDone();
    },
  });
  const fault = save.error instanceof ApiError ? save.error.faults().find((one) => one.path === 'amount')?.message : undefined;
  return (
    <form
      className="flex flex-wrap items-end gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (parsed !== null) save.mutate();
      }}
    >
      <Input
        label={t('caps.row.amount')}
        value={typed}
        onChange={(event) => setTyped(event.target.value)}
        inputMode="decimal"
        error={typed.trim() !== '' && parsed === null ? t(cap.meter === 'credits' ? 'caps.form.faults.credits' : cap.meter === 'tokens' ? 'caps.form.faults.tokens' : 'caps.form.faults.turns', { places: 6 }) : fault}
        fieldClassName="w-[10rem]"
        data-autofocus
      />
      <Button type="submit" size="sm" variant="primary" pending={save.isPending} disabled={parsed === null || parsed === cap.amount}>
        {t('caps.row.save')}
      </Button>
      <Button type="button" size="sm" variant="ghost" disabled={save.isPending} onClick={onDone}>
        {t('common.cancel')}
      </Button>
      {save.isError && fault === undefined && <ErrorAlert error={save.error} className="basis-full" />}
    </form>
  );
}

function CapRow({ status, here, readOnlyHere, metering, onRates }: { status: CapStatus; here: { nodeId?: string; providerId?: string }; readOnlyHere: boolean; metering: Metering | undefined; onRates: (() => void) | undefined }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const cap = status.cap;
  const [editing, setEditing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [showWaiting, setShowWaiting] = useState(false);

  const inheritedFrom = (): string | null => {
    if (readOnlyHere) return null;
    const here_ = here.providerId !== undefined ? cap.provider?.id === here.providerId : cap.node?.id === here.nodeId && cap.provider === null;
    if (here_) return null;
    if (cap.provider !== null) return t('caps.row.setOn', { name: cap.provider.name });
    return cap.node === null ? t('caps.row.setOnSystem') : t('caps.row.setOn', { name: cap.node.name ?? '' });
  };
  const inherited = inheritedFrom();
  const mayChange = cap.can_edit && inherited === null && !readOnlyHere;

  const switchOn = useMutation({
    mutationFn: (enabled: boolean) => updateCap(cap.id, { expected_revision: cap.revision, enabled }),
    onError: (error) => toast({ tone: 'danger', title: t('caps.row.notChanged'), description: error instanceof ApiError ? error.message : undefined }),
  });
  const remove = useMutation({
    mutationFn: () => deleteCap(cap.id),
    onSuccess: () => {
      setRemoving(false);
      toast({ tone: 'success', title: t('caps.remove.done') });
    },
  });

  const running = cap.meter === 'running';
  const amount = decimal(cap.amount);
  const percent = cap.amount === null || amount <= 0 ? 100 : Math.min(100, Math.round((decimal(status.used) / amount) * 100));
  const enabledShown = switchOn.isPending ? switchOn.variables : cap.enabled;
  const firm = metering === undefined || running ? null : t(`caps.firm.${metering}`);
  const target = cap.provider !== null ? ({ to: '/resources/providers/$id' as const, params: { id: cap.provider.id }, search: { tab: 'caps' as const } }) : cap.node !== null ? ({ to: '/usage' as const, search: { scope: `node:${cap.node.id}` } }) : null;

  const menu = [
    { id: 'edit', label: t('caps.row.edit'), icon: <Pencil aria-hidden />, onSelect: () => openCap({ edit: cap }) },
    { type: 'separator' as const, id: 'apart' },
    { id: 'remove', label: t('caps.row.remove'), icon: <Trash2 aria-hidden />, danger: true, onSelect: () => setRemoving(true) },
  ];

  return (
    <li aria-busy={switchOn.isPending} className={cx('flex flex-col gap-2 px-4 py-3', !cap.enabled && 'bg-bg-subtle')}>
      <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-1">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <p className="text-base font-medium wrap-anywhere text-text"><CapTitle cap={cap} /></p>
          <p className="flex flex-wrap items-center gap-x-2 text-sm text-text-muted">
            <CapSetter user={cap.created_by} />
            {inherited !== null && (
              <span className="flex items-center gap-1">
                <Badge>{t('caps.row.inherited')}</Badge>
                {target === null ? <span>{inherited}</span> : <Link {...target} className="text-accent-text hover:underline">{inherited}</Link>}
              </span>
            )}
            {status.person !== null && <CapShare user={status.person} />}
            {!cap.enabled && <Badge>{t('caps.row.off')}</Badge>}
          </p>
          {cap.note !== '' && <p className="text-sm text-text-muted">{cap.note}</p>}
        </div>
        <div className="flex items-center gap-2">
          {status.state !== 'ok' && cap.enabled && <Badge tone={status.state === 'reached' ? 'danger' : 'warning'}>{status.state === 'reached' ? t('caps.row.reached') : t('caps.row.warning')}</Badge>}
          {mayChange && <Switch label={t('caps.row.inForce')} checked={enabledShown} onCheckedChange={(on) => switchOn.mutate(on)} />}
          {mayChange && (
            <Menu
              label={t('caps.row.menu', { name: capTitle(cap, t) })}
              align="end"
              entries={menu}
              trigger={<IconButton label={t('caps.row.menu', { name: capTitle(cap, t) })} icon={<Ellipsis aria-hidden />} />}
            />
          )}
        </div>
      </div>

      {editing ? (
        <AmountEditor cap={cap} onDone={() => setEditing(false)} />
      ) : (
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-baseline justify-between gap-x-3 text-sm">
            <span className="text-text tabular-nums">{cap.amount === null ? allowanceWords(cap, t, locale) : usedWords(status, t, locale)}</span>
            <span className="flex items-center gap-2 text-text-muted">
              {cap.amount !== null && <span>{allowanceWords(cap, t, locale)}</span>}
              {mayChange && (
                <Button size="sm" variant="ghost" onClick={() => setEditing(true)}>
                  {t('caps.row.change')}
                </Button>
              )}
            </span>
          </div>
          <CapMeter status={status} label={t('caps.row.meter', { percent })} />
          {cap.period!==null && cap.period!=='total' && cap.amount!==null && <ForecastLine forecast={status.forecast} usedPercent={decimal(cap.amount)<=0 ? 100 : (decimal(status.used)+decimal(status.in_progress))/decimal(cap.amount)*100} resetsAt={status.period_ends_at} cap />}
        </div>
      )}

      <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
        {status.period_ends_at !== null && <span>{startsAgain(status.period_ends_at, cap.period, locale, t)}</span>}
        {firm !== null && <span>{firm}</span>}
        {decimal(status.in_progress) > 0 && <span>{t('caps.row.running', { amount: cap.meter === 'tokens' ? formatTokens(decimal(status.in_progress), locale) : formatCredits(decimal(status.in_progress), locale) })}</span>}
        {status.bypasses.count > 0 && (
          <Popover
            label={t('caps.row.passedWho')}
            trigger={
              <Button size="sm" variant="ghost">
                {t('caps.row.passed', { count: status.bypasses.count })}
              </Button>
            }
            className="w-[18rem] text-sm"
          >
            {status.bypasses.last === null ? (
              <p>{t('caps.row.passedNone')}</p>
            ) : (
              <p className="flex flex-wrap items-center gap-x-2">
                <span>{t('caps.row.passedLast')}</span>
                <UserChip user={status.bypasses.last.by} size={24} />
                <RelativeTime at={status.bypasses.last.at} className="text-text-muted" />
              </p>
            )}
          </Popover>
        )}
        {running && status.waiting > 0 && (
          <Button size="sm" variant="ghost" aria-expanded={showWaiting} onClick={() => setShowWaiting(!showWaiting)}>
            {t('caps.row.waiting', { count: status.waiting })}
          </Button>
        )}
      </p>
      {running && showWaiting && <Waiting capId={cap.id} />}

      {status.left_out.unrated_tokens > 0 && (
        <p className="flex flex-wrap items-baseline gap-x-2 text-sm text-text-muted">
          <span>{t('caps.row.leftOut', { tokens: formatTokens(status.left_out.unrated_tokens, locale) })}</span>
          {onRates !== undefined && (
            <Button size="sm" variant="ghost" onClick={onRates}>
              {t('usage.figures.addRates')}
            </Button>
          )}
        </p>
      )}

      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        title={t('caps.remove.title')}
        confirmLabel={t('caps.remove.confirm')}
        danger
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => remove.mutate()}
      >
        <p>{capSentence(cap, t, locale)}</p>
        <p className="mt-2 text-text-muted">{t('caps.remove.about')}</p>
      </ConfirmDialog>
    </li>
  );
}

function LimitsView({ of, metering, onRates, bare = false, className }: LimitsProps) {
  const { t } = useTranslation();
  const nodeReports = of.kind === 'node' && of.reports === true;
  // Whoever reads reports at a place sees every cap there, those switched off among them; everybody else what binds them.
  const node = useQuery({ ...placeLimitsQuery(of.kind === 'node' ? of.id : ''), enabled: of.kind === 'node' && !nodeReports });
  const nodeCaps = useQuery({ ...capsQuery({ kind: 'node', id: of.kind === 'node' ? of.id : '', covering: true }), enabled: nodeReports });
  const provider = useQuery({ ...capsQuery({ kind: 'provider', id: of.kind === 'provider' ? of.id : '' }), enabled: of.kind === 'provider' });
  const mine = useQuery({ ...myLimitsQuery, enabled: of.kind === 'mine' });
  const query = of.kind === 'node' ? (nodeReports ? nodeCaps : node) : of.kind === 'provider' ? provider : mine;
  const rows: CapStatus[] = (of.kind === 'node' ? (nodeReports ? nodeCaps.data : node.data?.caps) : of.kind === 'provider' ? provider.data : mine.data) ?? [];
  const here = of.kind === 'node' ? { nodeId: of.id } : of.kind === 'provider' ? { providerId: of.id } : {};
  const mayAdd = of.kind !== 'mine' && of.manage;

  const add = () => openCap(of.kind === 'node' ? { node: { id: of.id } } : of.kind === 'provider' ? { provider: { id: of.id, name: of.name } } : {});

  const body = query.isPending ? (
    <SkeletonRows rows={3} />
  ) : query.isError ? (
    <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
  ) : rows.length === 0 ? (
    <p className="px-4 py-2 text-sm text-text-muted">{t(of.kind==='mine' ? 'caps.limits.noneMine' : 'caps.limits.none')}</p>
  ) : (
    <ul className="flex flex-col divide-y divide-border">
      {rows.map((status) => (
        <CapRow key={`${status.cap.id}:${status.person?.id ?? ''}`} status={status} here={here} readOnlyHere={of.kind === 'mine'} metering={metering} onRates={onRates} />
      ))}
    </ul>
  );

  return (
    <Section id="usage-limits" title={t('caps.limits.title')} className={cx(!bare && boxClasses,'[&>div:first-child]:px-4 [&>div:first-child]:pt-3',className)} action={mayAdd ? <Button size="sm" icon={<Plus aria-hidden />} onClick={add}>{t('caps.limits.add')}</Button> : undefined}>
      {of.kind === 'mine' && <p className="px-4 pt-1 text-sm text-text-muted">{t('caps.limits.mineAbout')}</p>}
      <div className="pt-2 pb-1">
        {body}
      </div>
    </Section>
  );
}

/** The card is not drawn again for props that did not change: it asks for what it shows itself. */
export const Limits = memo(LimitsView);
