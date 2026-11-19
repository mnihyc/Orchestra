/** The system's own pattern rate card and guarded catalogue replacement, using the same rate form. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { catalogueQuery, nodeSettingsQuery, RATE_SETTINGS, ratesQuery, refreshRates, setCatalogue, setRateSetting } from '@/api/usage';
import type { AdminUsageCatalogueSetBody } from '@/generated/api-types';
import { RateDialog, type RateMode } from './RateForm';
import { cardOf, draftOf, emptyDraft, type RateDraft } from './rates';
import { CatalogueDialog } from './Catalogue';
import { Scale } from './RateCard';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';

/** The API admits at most two MiB of a catalogue; reject larger files before reading them. */
const CATALOGUE_MAX_BYTES = 2 * 1024 * 1024;
type RateKey = (typeof RATE_SETTINGS)[keyof typeof RATE_SETTINGS];

export function SystemRates() {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const settings = useQuery(nodeSettingsQuery(null));
  const table = useQuery(ratesQuery(null));
  const catalogue = useQuery(catalogueQuery());
  const [opened, setOpened] = useState<{ mode:RateMode;initial:RateDraft;replacing:string|null } | null>(null);
  const [seeing, setSeeing] = useState(false);
  const [file, setFile] = useState<File | null>(null);
  const row = (key:string) => settings.data?.find((setting) => setting.key === key)?.own;
  const card = cardOf(row(RATE_SETTINGS.card)?.value);
  const write = useMutation({ mutationFn:({key,value}:{ key:RateKey;value:unknown }) => setRateSetting(null,key,value,row(key)?.revision ?? null),onError:() => refreshRates(cache) });
  const replace = useMutation({ mutationFn:async (source:'fetch'|'shipped'|'file') => {
    let body:AdminUsageCatalogueSetBody;
    if (source === 'file') {
      if (file === null || file.size > CATALOGUE_MAX_BYTES) throw new Error(t('usageAdmin.fileSize'));
      body = JSON.parse(await file.text()) as AdminUsageCatalogueSetBody;
    } else body = source === 'fetch' ? { fetch:true } : { shipped:true };
    return setCatalogue(body);
  },onSuccess:() => { setFile(null); } });
  if (settings.isPending || table.isPending) return <SkeletonRows rows={4} />;
  if (settings.isError || table.isError) return <ErrorState error={settings.error ?? table.error} onRetry={() => { void settings.refetch(); void table.refetch(); }} />;
  return <div className="flex flex-col gap-6">
    <Section id="system-rate-card" title={t('usageAdmin.systemCard')} lead={t('usageAdmin.systemHelp')} action={<Button onClick={() => setOpened({ mode:'add',initial:emptyDraft(),replacing:null })}>{t('usage.rates.add')}</Button>}>
      <ul className="flex flex-col divide-y divide-border">{Object.entries(card).map(([pattern,rate]) => <li key={pattern} className="flex flex-wrap items-center justify-between gap-2 py-2"><span className="font-mono text-sm">{pattern}</span><span className="text-sm tabular-nums">{rate.input_per_mtok} / {rate.cached_input_per_mtok} / {rate.output_per_mtok}</span><Button size="sm" onClick={() => setOpened({ mode:'edit',initial:draftOf(pattern,rate),replacing:pattern })}>{t('usage.rates.edit')}</Button></li>)}</ul>
      {Object.keys(card).length === 0 && <EmptyState compact title={t('usageAdmin.noRates')} />}
      <Switch label={t('usage.rates.estimates')} checked={table.data.catalogue.on} disabled={write.isPending} onCheckedChange={(value) => write.mutate({ key:RATE_SETTINGS.estimates,value })} />
      <Scale key={table.data.catalogue.scale} node={{ id:null }} current={table.data.catalogue.scale} revision={row(RATE_SETTINGS.scale)?.revision ?? null} manage />
      {write.error && <ErrorAlert error={write.error} />}
      {opened !== null && <RateDialog node={{ id:null,name:t('usageAdmin.system') }} mode={opened.mode} initial={opened.initial} replacing={opened.replacing} card={card} revision={row(RATE_SETTINGS.card)?.revision ?? null} onClose={() => setOpened(null)} />}
    </Section>
    <Section id="rate-catalogue" title={t('usageAdmin.catalogue')}>
      {catalogue.data !== undefined && catalogue.data !== null && <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted"><span>{t('usage.rates.catalogueFrom',{date:catalogue.data.as_of,origin:t(`usageAdmin.origins.${catalogue.data.origin}`)})}</span><span>{t('usage.rates.catalogueLoaded')} <RelativeTime at={catalogue.data.loaded_at} /></span></p>}
      {catalogue.data === null && <EmptyState compact title={t('usage.rates.catalogue.none')} description={t('usage.rates.catalogue.noneAbout')} />}
      {catalogue.error && <ErrorAlert error={catalogue.error} />}
      <div className="flex flex-wrap gap-2"><Button onClick={() => setSeeing(true)}>{t('usage.rates.see')}</Button><Button pending={replace.isPending} onClick={() => replace.mutate('fetch')}>{t('usageAdmin.fetch')}</Button><Button disabled={replace.isPending} onClick={() => replace.mutate('shipped')}>{t('usageAdmin.shipped')}</Button></div>
      <Input type="file" accept="application/json,.json" label={t('usageAdmin.file')} onChange={(event) => setFile(event.target.files?.[0] ?? null)} disabled={replace.isPending} />
      <Button className="self-start" disabled={file === null || replace.isPending} onClick={() => replace.mutate('file')}>{t('usageAdmin.upload')}</Button>
      {replace.error && <ErrorAlert error={replace.error} />}
      <CatalogueDialog open={seeing} onOpenChange={setSeeing} />
    </Section>
  </div>;
}
