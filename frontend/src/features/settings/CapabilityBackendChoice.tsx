/** Every declared capability backend uses one compact provider/route control and one scoped supplier. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import { capabilityBackendsQuery, capabilityProviderQuery, selectedCapabilityBackendQuery } from '@/api/capabilityBackends';
import type { SettingsTarget } from '@/api/settings';
import type { BackendRef, CapabilityBackendCandidate } from '@/generated/api-types';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { ProviderDefaultChoice } from '@/features/settings/ProviderChoice';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { backendCapabilities, changeBackend, selectedBackend } from './capabilityBackendValue';
import { useDebounced } from '@/lib/debounce';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { ResourcePickerLookup } from '@/ui/ResourcePicker';

const hidden = (error: unknown) => error instanceof ApiError && [401, 403, 404].includes(error.status);
const identity = (backend: BackendRef) => JSON.stringify([backend.route, backend.offering?.provider, backend.offering?.offering]);

function useWords() {
  const { t, i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? i18n.language;
  const text = (words: Readonly<Record<string, string>> | undefined) => words?.[language] ?? words?.en;
  const route = (row: CapabilityBackendCandidate) => {
    const program = text(row.program_text);
    return program === undefined ? text(row.route_text) ?? t('capabilityRoutes.unknownRoute') : t('capabilityBackend.delegated', { program });
  };
  const reason = (row: CapabilityBackendCandidate) => text(row.reason_text) ?? (row.reason !== null && i18n.exists(`capabilityRoutes.reasons.${row.reason}`)
    ? t(`capabilityRoutes.reasons.${row.reason}`) : t('capabilityRoutes.unavailable'));
  return { t, route, reason };
}

export function CapabilityBackendChoice({ settingKey, target, ...props }: ValueControlProps & { settingKey: string; target: SettingsTarget }) {
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  if (target.kind !== 'node') return <CapabilityBackendSummary settingKey={settingKey} value={props.value} target={target} />;
  const capabilities = backendCapabilities(settingKey);
  return <div className="flex w-full min-w-0 flex-col gap-2" data-capability-backend>
    {capabilities.map((capability) => <BackendChoice key={`${reader}:${target.id}:${capability.id}`} {...props} reader={reader} place={target.id} capability={capability.id} title={capability.title} multi={capabilities.length > 1}
      selected={selectedBackend(props.value, capability.id)} onSelected={(backend) => { props.onEdited?.(); props.onChange(changeBackend(props.value, settingKey, capability.id, backend)); }} />)}
  </div>;
}

function BackendChoice({ reader, place, capability, title, multi, selected, onSelected, disabled = false, error }: ValueControlProps & {
  reader: string; place: string; capability: string; title: string; multi: boolean; selected: BackendRef | null; onSelected: (value: BackendRef) => void;
}) {
  const { t, route, reason } = useWords();
  const { i18n } = useTranslation();
  const input = useId();
  const frame = useRef<HTMLDivElement>(null), footer = useRef<HTMLDivElement>(null);
  const mounted = useRef(false), dismissed = useRef(0), requested = useRef('');
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const selectedProvider = selected?.offering?.provider ?? '';
  const [draftProvider, setDraftProvider] = useState<string | null>(null);
  const provider = draftProvider ?? selectedProvider;
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState('');
  const q = typed.trim(), settled = useDebounced(q), waiting = q !== settled;
  const current = useQuery({ ...selectedCapabilityBackendQuery(reader, selectedProvider, place, capability, selected?.offering?.offering ?? ''), enabled: selectedProvider !== '' });
  const list = useInfiniteQuery({ ...capabilityBackendsQuery(reader, provider, place, capability, settled), enabled: open && provider !== '' && !disabled && !waiting });
  const rows = waiting || hidden(list.error) || list.isError && !list.isFetchNextPageError ? [] : [...new Map((list.data?.pages.flatMap((page) => page.items) ?? []).map((row) => [identity(row.backend), row])).values()];
  const chosen = current.isError ? undefined : current.data?.items.find((row) => selected !== null && identity(row.backend) === identity(selected));
  const chosenLabel = provider !== selectedProvider || selected === null ? '' : chosen === undefined
    ? t(hidden(current.error) || current.isSuccess ? 'settings.compact.managed' : current.isPending ? 'common.loading' : 'capabilityBackend.unavailable')
    : `${route(chosen)} · ${chosen.offering_name}`;
  const field = t('capabilityBackend.route');
  const capName = i18n.exists(`capabilityRoutes.names.${capability}`) ? t(`capabilityRoutes.names.${capability}`) : title;
  const recover = (more: boolean) => {
    if (disabled || waiting || list.isFetching || more && !list.hasNextPage) return;
    const trigger = document.activeElement, dismissal = dismissed.current, query = settled;
    const task = more ? list.fetchNextPage({ cancelRefetch: false }) : list.refetch({ cancelRefetch: false });
    void task.then(() => {
      const session = client.session.getState();
      if (!mounted.current || `${session.userId ?? ''}:${session.readerVersion ?? 0}` !== reader || dismissed.current !== dismissal || requested.current !== query) return;
      const active = document.activeElement;
      if (active !== trigger && !frame.current?.contains(active) && !footer.current?.contains(active)
        && !(active === document.body && trigger instanceof HTMLElement && !trigger.isConnected)) return;
      setOpen(true); document.getElementById(input)?.focus();
    });
  };
  const problem = list.isError && (!list.isFetchNextPageError || hidden(list.error)) ? <ErrorState compact error={list.error} retrying={list.isFetching} onRetry={() => recover(false)} />
    : <Pagination hasMore={list.hasNextPage} loading={list.isFetching} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => recover(true)} />;
  return <div ref={frame} className="flex min-w-0 flex-col gap-1">
    {multi && <span className="text-xs text-text-muted">{capName}</span>}
    <div className="grid min-w-0 grid-cols-1 items-start gap-2 sm:grid-cols-2">
      <ProviderDefaultChoice shape={{ type: 'id' }} value={provider} label={t('capabilityBackend.provider')} disabled={disabled} hideLabel
        onChange={(next) => { if (typeof next === 'string') { setDraftProvider(next); requested.current = ''; setTyped(''); setOpen(false); } }} />
      <div className="flex min-w-0 flex-col gap-1" onKeyDownCapture={(event) => { if (event.key === 'Enter') event.preventDefault(); }}>
        <ResourcePickerLookup pickerKey={`capability-backend-${capability}`} id={input} size="sm" label={field} hideLabel disabled={disabled || provider === ''} error={error}
          query={typed} onQueryChange={(next) => { requested.current = next.trim(); setTyped(next); }} displayValue={chosenLabel} placeholder={t(provider === '' ? 'capabilityBackend.providerFirst' : 'capabilityBackend.choose')}
          open={open} onOpenChange={(next) => { if (!next) dismissed.current += 1; setOpen(next); }} loading={waiting || list.isFetching} initialActiveValue={selected === null ? undefined : identity(selected)}
          groups={[{ id: 'routes', label: '', options: rows.map((row) => ({ value: identity(row.backend), label: `${route(row)} · ${row.offering_name}`, text: `${route(row)} · ${row.offering_name}`,
            disabled: row.eligible ? false : reason(row) })) }]}
          footer={open && (list.isError || list.hasNextPage) ? <div ref={footer}>{problem}</div> : undefined}
          onSelect={(option) => {
            const row = rows.find((row) => identity(row.backend) === option.value);
            if (row === undefined || !row.eligible || waiting || list.isFetching || disabled) return;
            setOpen(false); requested.current = ''; setTyped('');
            onSelected(selected !== null && identity(selected) === identity(row.backend) ? selected : row.backend);
          }} />
        {!open && current.isError && !hidden(current.error) && <ErrorState compact title={t('capabilityBackend.unavailable')} error={current.error} retrying={current.isFetching} onRetry={() => void current.refetch()} />}
        {chosen !== undefined && !chosen.eligible && provider === selectedProvider && <span className="text-xs text-text-muted">{reason(chosen)}</span>}
      </div>
    </div>
  </div>;
}

export function CapabilityBackendSummary({ settingKey, value, target }: { settingKey: string; value: unknown; target?: SettingsTarget | undefined }) {
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const { t, i18n } = useTranslation();
  const capabilities = backendCapabilities(settingKey);
  return <span className="inline-flex min-w-0 flex-col gap-1">{capabilities.map((capability) => {
    const backend = selectedBackend(value, capability.id);
    return <span key={`${reader}:${capability.id}`} className="inline-flex min-w-0 flex-wrap items-center gap-x-1">
      {capabilities.length > 1 && <span className="text-text-muted">{i18n.exists(`capabilityRoutes.names.${capability.id}`) ? t(`capabilityRoutes.names.${capability.id}`) : capability.title} ·</span>}
      {backend?.offering === undefined || backend.offering === null ? <span>{t('settings.compact.none')}</span>
        : <BackendName reader={reader} provider={backend.offering.provider} capability={capability.id} backend={backend} place={target?.kind === 'node' ? target.id : ''} />}
    </span>;
  })}</span>;
}

function BackendName({ reader, provider, place, capability, backend }: { reader: string; provider: string; place: string; capability: string; backend: BackendRef }) {
  const { t, route, reason } = useWords();
  const account = useQuery(capabilityProviderQuery(reader, provider));
  const current = useQuery({ ...selectedCapabilityBackendQuery(reader, provider, place, capability, backend.offering?.offering ?? ''), enabled: place !== '' });
  const chosen = current.isError ? undefined : current.data?.items.find((row) => identity(row.backend) === identity(backend));
  if (hidden(account.error) || account.data === null || hidden(current.error) || current.isSuccess && chosen === undefined) return <span className="text-text-muted">{t('settings.compact.managed')}</span>;
  if (account.isError || current.isError) return <ErrorState compact title={t('capabilityBackend.unavailable')} error={account.error ?? current.error} onRetry={() => { void account.refetch(); if (place !== '') void current.refetch(); }} />;
  if (account.data === undefined || place !== '' && chosen === undefined) return <span className="text-text-muted">{t('common.loading')}</span>;
  return <span className="inline-flex min-w-0 flex-wrap items-center gap-x-1 text-sm">
    <ResourceName subject={{ kind: 'provider', provider: account.data }} />
    {chosen !== undefined && <><span>· {route(chosen)} · {chosen.offering_name}</span>{!chosen.eligible && <span className="text-text-muted">· {reason(chosen)}</span>}</>}
  </span>;
}
