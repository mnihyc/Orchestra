/** Declared services are opt-in; these controls use the existing provider writer and reviewed revision. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { providerKey, updateProvider } from '@/api/resources';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { EditDialog } from '@/features/resources/EditDialog';
import { groupFault } from '@/features/resources/providers/entry';
import type { Provider } from '@/generated/api-types';
import { holds, denial } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { adminSpec } from '@/ui/AdminAct';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { StateLine } from '@/ui/StateLine';
import { toast } from '@/ui/Toast';

type Offering = NonNullable<Provider['service_catalogue']>[number];
type Config = NonNullable<Provider['service_offerings']>[string];
type PriceMode = 'catalogue' | 'credits' | 'saved';
interface PriceDraft { mode: PriceMode; amount: string }
const decimal = /^\d+(?:\.\d+)?$/u;
const AMOUNT_MAX_CHARS = 40;

function ServiceDialog({ provider, offering, onClose }: { provider: Provider; offering: Offering; onClose: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const unitName = (unit: string) => t(`providerServices.units.${unit}`, { defaultValue: unit });
  const currencyName = (currency: string | undefined) => currency === 'credits' ? t('providerServices.currencyCredits') : currency ?? '';
  const original: Config = provider.service_offerings?.[offering.id] ?? { enabled: false, prices: {} };
  const [enabled, setEnabled] = useState(original.enabled ?? false);
  const [inheritGroups, setInheritGroups] = useState(original.groups === undefined || original.groups === null);
  const [groupsText, setGroupsText] = useState((original.groups ?? offering.declared_groups).join(', '));
  const [drafts, setDrafts] = useState<Record<string, PriceDraft>>(() => Object.fromEntries(offering.units.map((unit) => {
    const saved = original.prices?.[unit];
    return [unit, { mode: saved === undefined ? 'catalogue' : saved.currency === 'credits' ? 'credits' : 'saved', amount: saved?.currency === 'credits' ? saved.amount ?? '' : '' }];
  })));
  const groups = [...new Set(groupsText.split(',').map((name) => name.trim()).filter(Boolean))];
  const groupError = inheritGroups ? undefined : groups.map((name) => groupFault(name, t)).find((fault) => fault !== null) ?? undefined;
  const prices = { ...original.prices };
  let valid = groupError === undefined;
  for (const unit of offering.units) {
    const draft = drafts[unit];
    if (draft === undefined) continue;
    if (draft.mode === 'catalogue') delete prices[unit];
    else if (draft.mode === 'credits') {
      const amount = draft.amount.trim();
      if (!decimal.test(amount) || amount.length > AMOUNT_MAX_CHARS) valid = false;
      prices[unit] = { ...original.prices?.[unit], amount, currency: 'credits', unit };
    }
  }
  const next: Config = { ...original, enabled, prices };
  if (inheritGroups) delete next.groups;
  else next.groups = groups;
  const changed = JSON.stringify(next) !== JSON.stringify(original);
  const save = useMutation({
    mutationFn: () => updateProvider(provider.id, { expected_revision: provider.revision, service_offerings: { ...provider.service_offerings, [offering.id]: next } }),
    onSuccess: () => { void client.invalidateQueries({ queryKey: providerKey(provider.id) }); toast({ tone: 'success', title: t('providerServices.saved') }); onClose(); },
    onError: () => void client.invalidateQueries({ queryKey: providerKey(provider.id) }),
  });
  return <EditDialog admin={adminSpec(provider, 'provider.manage')} title={t('providerServices.edit')} description={offering.title}
    changed={changed} valid={valid} pending={save.isPending} error={save.error} onSave={() => save.mutate()} onClose={onClose}>
    <Checkbox label={t('providerServices.enable')} checked={enabled} onCheckedChange={setEnabled} disabled={save.isPending || !offering.available && !enabled} />
    {!offering.available && <StateLine compact message={t('providerServices.unavailable')} />}
    <Checkbox label={t('providerServices.inheritGroups')} checked={inheritGroups} onCheckedChange={setInheritGroups} disabled={save.isPending} />
    <Input label={t('providerServices.groups')} value={groupsText} onChange={(event) => setGroupsText(event.target.value)} disabled={save.isPending || inheritGroups} error={groupError} />
    {offering.units.map((unit) => {
      const draft = drafts[unit] ?? { mode: 'catalogue' as const, amount: '' };
      const saved = original.prices?.[unit];
      const quote = offering.catalogue_prices[unit];
      const options: { value: PriceMode; label: string }[] = [
        { value: 'catalogue', label: t('providerServices.cataloguePrice') },
        { value: 'credits', label: t('providerServices.creditsPrice') },
      ];
      if (saved !== undefined && saved.currency !== 'credits') options.push({ value: 'saved', label: t('providerServices.savedPrice', { currency: saved.currency ?? '' }) });
      return <fieldset key={unit} className="flex min-w-0 flex-col gap-2 border-t border-border pt-3">
        <Select label={t('providerServices.price', { unit: unitName(unit) })} value={draft.mode} options={options} disabled={save.isPending}
          onValueChange={(mode) => setDrafts({ ...drafts, [unit]: { ...draft, mode } })} />
        {quote !== undefined && <p className="text-sm text-text-muted">{t('providerServices.estimate')}: {quote.amount} {currencyName(quote.currency)} / {unitName(unit)}</p>}
        {draft.mode === 'saved' && <p className="text-sm text-text-muted">{saved?.amount} {currencyName(saved?.currency)} / {unitName(unit)}</p>}
        {draft.mode === 'credits' && <Input label={t('providerServices.credits', { unit: unitName(unit) })} value={draft.amount} inputMode="decimal" disabled={save.isPending}
          error={decimal.test(draft.amount.trim()) && draft.amount.trim().length <= AMOUNT_MAX_CHARS ? undefined : t('providerServices.invalidAmount')}
          onChange={(event) => setDrafts({ ...drafts, [unit]: { ...draft, amount: event.target.value } })} />}
      </fieldset>;
    })}
  </EditDialog>;
}

export function ProviderServices({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<string | null>(null);
  const services = provider.service_catalogue ?? [];
  if (provider.kind !== 'api' || provider.service_catalogue === undefined || provider.service_catalogue === null) return null;
  const manage = holds(provider.access, 'provider.manage');
  const selected = services.find((service) => service.id === editing);
  return <Section id="provider-services" title={t('providerServices.title')}>
    <div className="flex justify-end"><ArticleGuideLink article="provider-details" heading="service-backends" title={t('providerServices.guide')} /></div>
    {services.length === 0 ? <StateLine compact message={t('providerServices.empty')} /> : <ul className="divide-y divide-border">
      {services.map((service) => <li key={service.id} className="flex min-w-0 flex-wrap items-start justify-between gap-3 py-3" data-provider-service={service.id}>
        <div className="min-w-0 flex-1 basis-48">
          <h3 className="font-medium wrap-anywhere">{service.title}</h3>
          <p className="text-sm text-text-muted">{t(service.enabled ? 'providerServices.enabled' : 'providerServices.disabled')}</p>
          {!service.available && <StateLine compact message={t('providerServices.unavailable')} />}
          <p className="text-sm text-text-muted">{service.groups.map((group) => `@${group}`).join(', ')}</p>
          {Object.entries(service.prices).map(([unit, price]) => <p key={unit} className="text-sm text-text-muted">{t(service.explicit_prices.includes(unit) ? 'providerServices.providerPrice' : 'providerServices.estimate')}: {price.amount} {price.currency === 'credits' ? t('providerServices.currencyCredits') : price.currency} / {t(`providerServices.units.${unit}`, { defaultValue: unit })}</p>)}
        </div>
        <Button size="sm" variant="ghost" action="edit" admin={adminSpec(provider, 'provider.manage')} blocked={!manage} reason={denial(t, 'provider.manage', provider.name, provider.owner)}
          onClick={() => setEditing(service.id)}>{t('providerServices.edit')}</Button>
      </li>)}
    </ul>}
    {selected !== undefined && <ServiceDialog key={`${provider.id}:${selected.id}`} provider={provider} offering={selected} onClose={() => setEditing(null)} />}
  </Section>;
}
