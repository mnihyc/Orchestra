/** The rate card of a place: the credits a million tokens of each kind cost for a model, from which day; what the shipped catalogue estimates where nobody entered a rate, marked as an estimate; and the models in use that have no rate.
 *
 * Nobody has to fill this table in before a figure in credits appears: a model that nobody rated is
 * estimated from the catalogue of model families that ships with Orch, and each row says where its rate
 * comes from, in the order it is looked for: the provider's own, the card of this place or one above it,
 * an estimate with the pattern of the family that matched, or nothing. No rate reads "–", never 0. The
 * models that were used here without one come first, with how many tokens, so that the table is filled
 * from what is missing. Whoever may write the rate card of an org adds a rate, changes a row, copies an
 * estimate to the card (from then on that family follows the card and not the catalogue), switches
 * estimates off and says what one unit of the catalogue is worth; nobody changes the catalogue itself.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { BookOpen, Coins } from 'lucide-react';
import { useState } from 'react';
import { modelDisplayName } from '@/features/models/names';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { catalogueQuery, nodeSettingsQuery, RATE_SETTINGS, ratesQuery, refreshRates, setRateSetting } from '@/api/usage';
import type { NodeKind, RateTable, RateTableModel } from '@/generated/api-types';
import { CatalogueDialog, creditsPerMtok } from '@/features/usage/Catalogue';
import { ESTIMATE_MARK } from '@/features/usage/figures';
import { type Card, cardOf, draftOf, emptyDraft, isScale, type RateDraft } from '@/features/usage/rates';
import { type RateMode, RateDialog } from '@/features/usage/RateForm';
import { dayWords } from '@/features/usage/words';
import { useFormatContext } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';
import { boxClasses } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { type Column, Table } from '@/ui/Table';

export interface RateCardProps {
  node: { id: string; name: string; kind: NodeKind };
  /** Whether the viewer may write the rate card here: `settings.manage` on an org. */
  manage: boolean;
  /** Drawn without its own box and heading, inside a dialog that has them. */
  bare?: boolean;
  className?: string;
}

/** The routing name a pattern for a model that has no rate starts from: lower case, without what precedes its last slash. */
const patternFor = (modelName: string): string => (modelName.toLowerCase().split('/').at(-1) ?? '').trim();

interface Opened {
  mode: RateMode;
  initial: RateDraft;
  replacing: string | null;
}

/** What the switch for estimates and the worth of a unit of the catalogue are, and the line that says where the catalogue came from. */
function Estimates({ node, table, manage, revisions, bare }: { node: RateCardProps['node']; table: RateTable; manage: boolean; revisions: { estimates: number | null; scale: number | null }; bare: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const queryClient = useQueryClient();
  const catalogue = useQuery(catalogueQuery());
  const [seeing, setSeeing] = useState(false);

  const switchEstimates = useMutation({
    mutationFn: (on: boolean) => setRateSetting(node.id, RATE_SETTINGS.estimates, on, revisions.estimates),
    onError: () => refreshRates(queryClient),
  });
  const on = switchEstimates.isPending ? switchEstimates.variables : table.catalogue.on;

  // A system with no loaded catalogue gives the table an empty date and the catalogue read null.
  const none = table.catalogue.as_of === '';
  const origin = (() => {
    const loaded = catalogue.data;
    if (loaded === undefined || loaded === null) return null;
    const date = dayWords(loaded.loaded_at, locale);
    if (loaded.origin === 'shipped') return t('usage.rates.origin.shipped');
    if (loaded.origin === 'uploaded') return t('usage.rates.origin.uploaded', { name: loaded.loaded_by?.display_name ?? '', date });
    return t('usage.rates.origin.fetched', { source: loaded.source ?? '', date });
  })();

  return (
    <div className={cx('flex flex-col gap-3 pt-2', !bare && 'px-4')}>
      <Switch
        label={t('usage.rates.estimates')}
        hint={on ? t('usage.rates.estimatesOn') : t('usage.rates.estimatesOff')}
        checked={on}
        disabled={!manage || switchEstimates.isPending}
        onCheckedChange={(next) => switchEstimates.mutate(next)}
      />
      {switchEstimates.isError && <ErrorAlert error={switchEstimates.error} />}
      <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
        <span>{none ? t('usage.rates.catalogueNone') : origin === null ? t('usage.rates.catalogueOf', { date: dayWords(table.catalogue.as_of, locale) }) : t('usage.rates.catalogueFrom', { date: dayWords(table.catalogue.as_of, locale), origin })}</span>
        <Button size="sm" variant="ghost" icon={<BookOpen aria-hidden />} onClick={() => setSeeing(true)}>
          {t('usage.rates.see')}
        </Button>
      </p>
      <Scale key={table.catalogue.scale} node={node} current={table.catalogue.scale} manage={manage} revision={revisions.scale} />
      <CatalogueDialog open={seeing} onOpenChange={setSeeing} />
    </div>
  );
}

/** What one unit of the catalogue is worth here, in credits: one number, changed in place. */
export function Scale({ node, current, manage, revision }: { node: { id:string | null }; current: string; manage: boolean; revision: number | null }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [typed, setTyped] = useState(current);
  const write = useMutation({
    mutationFn: () => setRateSetting(node.id, RATE_SETTINGS.scale, typed.trim(), revision),
    onError: () => refreshRates(queryClient),
  });
  const valid = isScale(typed);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (valid && manage) write.mutate();
      }}
    >
      <div className="flex max-w-[28rem] items-start gap-2">
        <Input
          label={t('usage.rates.scale')}
          hint={t('usage.rates.scaleHint')}
          value={typed}
          readOnly={!manage}
          inputMode="decimal"
          onChange={(event) => setTyped(event.target.value)}
          error={typed.trim() !== '' && !valid ? t('usage.rates.faults.scale') : write.error instanceof ApiError ? write.error.faults().find((one) => one.path === 'value')?.message : undefined}
          fieldClassName="min-w-0 flex-1"
        />
        {manage && <Button type="submit" className="mt-6 shrink-0" variant="primary" pending={write.isPending} disabled={!valid || typed.trim() === current}>
          {t('usage.rates.scaleSave')}
        </Button>}
      </div>
      {write.isError && !(write.error instanceof ApiError && write.error.faults().length > 0) && <ErrorAlert error={write.error} className="mt-2" />}
    </form>
  );
}

/** A price that may be missing, as a reader says it. */
function creditsOrDash(price: string | null | undefined, locale: string): string {
  return price === undefined || price === null ? '–' : creditsPerMtok(price, locale);
}

/** The steps for large prompts as a reader says them: "over 272k: 2.5 / 15". */
function stepsOf(row: RateTableModel, locale: string, t: (key: 'usage.rates.above', options: { size: string; input: string; output: string }) => string): string {
  const steps = row.rate?.above ?? [];
  return steps.length === 0 ? '–' : steps.map((step) => t('usage.rates.above', { size: formatTokens(step.context_tokens, locale), input: creditsPerMtok(step.input_per_mtok, locale), output: creditsPerMtok(step.output_per_mtok, locale) })).join('; ');
}

/** The speeds of a rate as a reader says them: "fast ×2". */
function speedsOf(row: RateTableModel, locale: string, t: (key: 'usage.rates.speed', options: { tier: string; multiplier: string }) => string): string {
  const tiers = Object.entries(row.rate?.tiers ?? {});
  return tiers.length === 0 ? '–' : tiers.map(([tier, multiplier]) => t('usage.rates.speed', { tier, multiplier: creditsPerMtok(multiplier, locale) })).join(', ');
}

export function RateCard({ node, manage, bare = false, className }: RateCardProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const rates = useQuery(ratesQuery(node.id));
  const settings = useQuery({ ...nodeSettingsQuery(node.id), enabled: manage });
  const [opened, setOpened] = useState<Opened | null>(null);

  const row = (key: string) => settings.data?.find((one) => one.key === key)?.own ?? null;
  const card: Card = cardOf(row(RATE_SETTINGS.card)?.value);
  const revisions = { card: row(RATE_SETTINGS.card)?.revision ?? null, estimates: row(RATE_SETTINGS.estimates)?.revision ?? null, scale: row(RATE_SETTINGS.scale)?.revision ?? null };
  const ready = !manage || settings.isSuccess;

  const open = (mode: RateMode, source: RateTableModel) => {
    const match = source.match ?? patternFor(source.model_name);
    const mine = card[match];
    const base = mine ?? source.rate;
    if (mode === 'add') setOpened({ mode, initial: emptyDraft(patternFor(source.model_name)), replacing: null });
    else setOpened({ mode, initial: base === null ? emptyDraft(match) : draftOf(match, base), replacing: mine === undefined ? null : match });
  };

  const columns: Column<RateTableModel>[] = [
    {
      id: 'model',
      header: t('usage.rates.col.model'),
      // What a column that does not fit would have said is told under the model's name.
      cell: (model, shown) => {
        const extras = [
          ...(shown.has('cacheWrite') || model.rate?.cache_write_per_mtok === undefined || model.rate.cache_write_per_mtok === null ? [] : [t('usage.rates.cacheWriteIs', { price: creditsPerMtok(model.rate.cache_write_per_mtok, locale) })]),
          ...(shown.has('speeds') || speedsOf(model, locale, t) === '–' ? [] : [speedsOf(model, locale, t)]),
          ...(shown.has('large') || stepsOf(model, locale, t) === '–' ? [] : [stepsOf(model, locale, t)]),
        ];
        return (
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-text">{modelDisplayName(model.display_name,t)}</span>
            {model.provider.name !== model.display_name && <span className="truncate text-sm text-text-muted">{model.provider.name}</span>}
            {extras.map((value,index)=><span key={index} className="text-sm text-text-muted">{value}</span>)}
          </span>
        );
      },
    },
    { id: 'input', header: t('usage.rates.col.input'), align: 'end', cell: (model) => (model.rate === null ? '–' : creditsPerMtok(model.rate.input_per_mtok, locale)) },
    { id: 'cached', header: t('usage.rates.col.cached'), align: 'end', from: 'sm', cell: (model) => (model.rate === null ? '–' : creditsPerMtok(model.rate.cached_input_per_mtok, locale)) },
    { id: 'output', header: t('usage.rates.col.output'), align: 'end', cell: (model) => (model.rate === null ? '–' : creditsPerMtok(model.rate.output_per_mtok, locale)) },
    { id: 'cacheWrite', header: t('usage.rates.col.cacheWrite'), align: 'end', from: 'lg', cell: (model) => creditsOrDash(model.rate?.cache_write_per_mtok, locale) },
    { id: 'speeds', header: t('usage.rates.col.speeds'), from: 'lg', cell: (model) => speedsOf(model, locale, t) },
    { id: 'large', header: t('usage.rates.col.large'), from: 'lg', cell: (model) => stepsOf(model, locale, t) },
    {
      id: 'from',
      header: t('usage.rates.col.from'),
      cell: (model) => (
        <span className="flex flex-col items-start gap-1">
          <span className="text-text">
            {model.source === 'provider'
              ? t('usage.rates.from.provider')
              : model.source === 'rate_card'
                ? t('usage.rates.from.card', { match: model.match ?? '' })
                : model.source === 'catalogue'
                  ? `${ESTIMATE_MARK} ${t('usage.rates.from.catalogue', { match: model.match ?? '' })}`
                  : t('usage.rates.from.none')}
          </span>
          {model.source === null && model.unrated_tokens > 0 && <span className="text-sm text-text-muted">{t('usage.rates.unratedTokens', { tokens: formatTokens(model.unrated_tokens, locale) })}</span>}
          {model.source === 'provider' ? (
            <Button size="sm" variant="ghost" className="-ml-2" asChild>
              <Link to="/resources/providers/$id" params={{ id: model.provider.id }}>
                {t('usage.rates.openProvider')}
              </Link>
            </Button>
          ) : !manage ? null : model.source === null ? (
            <Button size="sm" disabled={!ready} onClick={() => open('add', model)}>
              {t('usage.rates.add')}
            </Button>
          ) : model.source === 'rate_card' ? (
            <Button size="sm" disabled={!ready} onClick={() => open('edit', model)}>
              {t('usage.rates.edit')}
            </Button>
          ) : (
            <Button size="sm" disabled={!ready} onClick={() => open('copy', model)}>
              {t('usage.rates.copy')}
            </Button>
          )}
        </span>
      ),
    },
  ];

  const unratedFirst = (rates.data?.models ?? []).toSorted((one, other) => Number(other.source === null) - Number(one.source === null) || other.unrated_tokens - one.unrated_tokens || one.display_name.localeCompare(other.display_name, locale));

  const body = rates.isPending ? (
    <SkeletonRows rows={4} />
  ) : rates.isError ? (
    <ErrorState compact error={rates.error} retrying={rates.isRefetching} onRetry={() => void rates.refetch()} />
  ) : (
    <div className="flex flex-col gap-3">
      <Estimates node={node} table={rates.data} manage={manage} revisions={revisions} bare={bare} />
      <div className={cx('flex flex-col gap-2', !bare && 'px-4')}>
        <p className="text-sm text-text-muted">{t('usage.rates.unit')}</p>
        {unratedFirst.length === 0 ? (
          <EmptyState compact icon={<Coins aria-hidden />} title={t('usage.rates.none')} description={t('usage.rates.noneAbout')} />
        ) : (
          <Table label={t('usage.rates.table')} columns={columns} rows={unratedFirst} rowKey={(model) => `${model.provider.id}/${model.model_name}`} />
        )}
        {!manage && node.kind !== 'org' && <p className="text-sm text-text-muted">{t('usage.rates.setAtOrg')}</p>}
        <p className="max-w-prose text-sm text-text-muted">{t('usage.rates.note')}</p>
      </div>
    </div>
  );

  return (
    <section aria-label={t('usage.rates.label')} className={cx(!bare && boxClasses, 'flex flex-col', !bare && 'pb-3', className)}>
      {!bare && (
        <div className="flex flex-col gap-1 px-4 pt-3">
          <h3 className="text-base font-medium text-text">{t('usage.rates.title', { name: node.name })}</h3>
          <p className="text-sm text-text-muted">{t('usage.rates.lead')}</p>
        </div>
      )}
      <div className={cx(!bare && 'pt-2')}>{body}</div>
      {opened !== null && <RateDialog node={node} mode={opened.mode} initial={opened.initial} replacing={opened.replacing} card={card} revision={revisions.card} onClose={() => setOpened(null)} />}
    </section>
  );
}

/** The rates of a place in a dialog of their own, opened from the figures that lack one and from the foot of the usage screen. */
export function RatesDialog({ open, onOpenChange, node, manage }: { open: boolean; onOpenChange: (open: boolean) => void; node: RateCardProps['node']; manage: boolean }) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="lg" title={t('usage.rates.title', { name: node.name })} description={t('usage.rates.lead')}>
      {open && <RateCard node={node} manage={manage} bare />}
    </Dialog>
  );
}
