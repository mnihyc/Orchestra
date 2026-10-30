/** The catalogue of estimates, read-only: its families with their vendor, their rates, the page they were read from and the day, and a field that says which family a model's name would match.
 *
 * The estimates Orch ships are list prices by family of model, in the catalogue's own unit. Nobody edits
 * them here: a family whose figure is wrong is copied to the rate card of a place, and from then on follows
 * the card. The field answers for a model that is not in use yet by the rule the server matches by: the
 * name in lower case, against each pattern, the most particular winning.
 */
import { useQuery } from '@tanstack/react-query';
import { BookOpen } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { catalogueQuery } from '@/api/usage';
import type { RateFamily } from '@/generated/api-types';
import { dayWords } from '@/features/usage/words';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Skeleton } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';

/** A rate in credits for a million tokens, as a reader says it: no places that are not there. */
export function creditsPerMtok(text: string, locale: string): string {
  const value = Number(text);
  return Number.isFinite(value) ? formatNumber(value, locale, { maximumFractionDigits: 6 }) : text;
}

/** Whether a source is an address a person can follow. */
const isAddress = (source: string): boolean => /^https?:\/\//.test(source);

/** An empty successful read has no catalogue to show. */
function NoCatalogue() {
  const { t } = useTranslation();
  return <EmptyState compact icon={<BookOpen aria-hidden />} title={t('usage.rates.catalogue.none')} description={t('usage.rates.catalogue.noneAbout')} />;
}

function Families({ families, unit }: { families: readonly RateFamily[]; unit: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const columns: Column<RateFamily>[] = [
    {
      id: 'family',
      header: t('usage.rates.catalogue.family'),
      cell: (family) => (
        <span className="flex flex-col">
          <span className="font-mono text-sm text-text">{family.match}</span>
          {family.note !== undefined && family.note !== null && family.note !== '' && <span className="text-sm text-text-muted">{family.note}</span>}
        </span>
      ),
    },
    { id: 'vendor', header: t('usage.rates.catalogue.vendor'), from: 'md', cell: (family) => family.vendor },
    { id: 'input', header: t('usage.rates.col.input'), align: 'end', cell: (family) => creditsPerMtok(family.rate.input_per_mtok, locale) },
    { id: 'cached', header: t('usage.rates.col.cached'), align: 'end', from: 'sm', cell: (family) => creditsPerMtok(family.rate.cached_input_per_mtok, locale) },
    { id: 'output', header: t('usage.rates.col.output'), align: 'end', cell: (family) => creditsPerMtok(family.rate.output_per_mtok, locale) },
    { id: 'asOf', header: t('usage.rates.catalogue.asOf'), from: 'md', cell: (family) => dayWords(family.as_of, locale) },
    {
      id: 'source',
      header: t('usage.rates.catalogue.source'),
      from: 'lg',
      cell: (family) =>
        isAddress(family.source) ? (
          <a href={family.source} target="_blank" rel="noreferrer" className="break-all text-accent-text hover:underline">
            {family.source}
          </a>
        ) : (
          <span className="break-all">{family.source}</span>
        ),
    },
  ];
  if (families.length === 0) return <NoCatalogue />;
  return (
    <div className="flex flex-col gap-2">
      <p className="text-sm text-text-muted">{t('usage.rates.catalogue.unit', { unit })}</p>
      <Table label={t('usage.rates.catalogue.table')} columns={columns} rows={families} rowKey={(family) => family.match} />
    </div>
  );
}

/** What the name that was typed would match, in a sentence. */
function Matching({ asked }: { asked: string }) {
  const { t } = useTranslation();
  const query = useQuery(catalogueQuery(asked));
  if (query.isPending) return <Skeleton className="h-[1.5rem] w-[16rem]" />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === null) return <NoCatalogue />;
  const family = query.data.families[0];
  return (
    <p role="status" className="text-sm text-text">
      {family === undefined ? t('usage.rates.catalogue.matchesNone', { name: asked }) : t('usage.rates.catalogue.matches', { name: asked, match: family.match, vendor: family.vendor })}
    </p>
  );
}

function Body() {
  const { t } = useTranslation();
  const query = useQuery(catalogueQuery());
  const [typed, setTyped] = useState('');
  const [asked, setAsked] = useState('');
  if (query.isPending) return <Skeleton className="h-[12rem] w-full" />;
  if (query.isError) return <ErrorState error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === null) return <NoCatalogue />;
  return (
    <div className="flex flex-col gap-4">
      <form
        className="flex max-w-[26rem] items-start gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          setAsked(typed.trim().toLowerCase());
        }}
      >
        <Input
          label={t('usage.rates.catalogue.name')}
          hint={t('usage.rates.catalogue.nameHint')}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          fieldClassName="min-w-0 flex-1"
        />
        <Button type="submit" className="mt-6 shrink-0" disabled={typed.trim() === ''}>
          {t('usage.rates.catalogue.look')}
        </Button>
      </form>
      {asked !== '' && <Matching asked={asked} />}
      <Families families={query.data.families} unit={query.data.unit} />
    </div>
  );
}

export function CatalogueDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="lg" title={t('usage.rates.catalogue.title')} description={t('usage.rates.catalogue.about')}>
      {open && <Body />}
    </Dialog>
  );
}
