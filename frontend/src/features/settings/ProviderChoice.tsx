/** A provider default is chosen and read by its authorized name inside the existing settings row. */
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { ApiError, api, client, ok } from '@/api/client';
import { everythingQuery } from '@/api/everything';
import { providerKey, providerQuery } from '@/api/resources';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { ResourceCreateButton } from '@/features/resources/ResourceCreation';
import { ResourceHint, ResourceName, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { providerLabels } from '@/features/resources/providers/labels';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import type { Provider } from '@/generated/api-types';
import { choiceOption } from '@/i18n/choices';
import { useDebounced } from '@/lib/debounce';
import { holds } from '@/lib/permissions';
import { isComposing } from '@/lib/shortcuts';
import { Combobox, type ComboboxOption } from '@/ui/Combobox';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';

/** Read-only references never ask a person to interpret a UUID or guess a hidden provider's name. */
export function ProviderDefaultName({ value, children }: { value: unknown; children?: (provider: Provider) => ReactNode }) {
  const { t } = useTranslation();
  return typeof value === 'string' && value !== '' ? <NamedProvider id={value} render={children} /> : <span className="text-text-muted">{t('settings.compact.none')}</span>;
}

// Native cache withdrawals can remove a row while its HTTP DTO remains non-nullable.
const withdrawn = (row: unknown) => row === null;

function NamedProvider({ id, render }: { id: string; render: ((provider: Provider) => ReactNode) | undefined }) {
  const { t } = useTranslation();
  const provider = useQuery({ ...providerQuery(id), staleTime: Infinity });
  if (withdrawn(provider.data) || provider.isError && provider.error instanceof ApiError && (provider.error.status === 403 || provider.error.status === 404)) {
    return <span className="text-sm text-text-muted">{t('settings.compact.managed')}</span>;
  }
  if (provider.isError) return <ErrorState compact title={t('places.providerDefault.unavailable')} error={provider.error} retrying={provider.isRefetching} onRetry={() => void provider.refetch()} />;
  if (provider.data !== undefined) return render === undefined ? <ResourceName subject={{ kind: 'provider', provider: provider.data }} /> : render(provider.data);
  return <span className="text-text-muted">{t('places.providerDefault.loading')}</span>;
}

const readerScope = () => { const state = client.session.getState(); return `${state.userId ?? ''}:${state.readerVersion ?? 0}`; };
const refused = (error: unknown) => error instanceof ApiError && [401, 403, 404].includes(error.status);

/** The parent's revision/lock/autosave machinery remains the only writer of the setting. */
export function ProviderDefaultChoice(props: ValueControlProps) {
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  return <ReaderChoice key={reader} {...props} reader={reader} />;
}

function ReaderChoice({ value, label, onChange, onEdited, error, disabled = false, placeholder, reader, showLabel }: ValueControlProps & { reader: string }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const known = useKnownPlaces({ trees: false });
  const session = useId();
  const frame = useRef<HTMLDivElement>(null), footer = useRef<HTMLDivElement>(null);
  const mounted = useRef(false), restoring = useRef(false), currentValue = useRef(value);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { currentValue.current = value; }, [value]);
  const [open, setOpen] = useState(false), [visited, setVisited] = useState(false), [typed, setTyped] = useState('');
  const [listRecovery, setListRecovery] = useState<{ query: string; error: unknown; more: boolean } | null>(null);
  const [currentRecovery, setCurrentRecovery] = useState<{ id: string; error: unknown } | null>(null);
  const text = typed.trim(), settled = useDebounced(text), waiting = text !== settled;
  const requested = useRef(''), visible = useRef(false), dismissed = useRef(0);
  const show = (next: boolean) => { visible.current = next; setOpen(next); };
  const type = (next: string) => { requested.current = next.trim(); setTyped(next); };
  // The caller's full usable reach is native: no home/member-root restriction,
  // no browser filtering of a first window, and no name-only/path-only choices.
  const source = everythingQuery(`${reader}:${session}`, 'resources', {
    scope: 'mine', kinds: ['provider'], q: settled, owner: null, createdFrom: '', createdTo: '', sort: 'name', direction: 'asc',
    resource: { details: true, view: 'usable' },
  }, 'provider-default-choices');
  const list = useInfiniteQuery({ ...source, enabled: open && !disabled && !waiting, retry: false, staleTime: Infinity,
    refetchOnMount: false, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const selected = typeof value === 'string' ? value : '';
  // Keep names across the parent's acknowledged-save remount, but never across
  // readers. Do not seed this exact read from an unscoped historical cache.
  const currentKey = (id: string) => [...providerKey(id), 'default-choice', reader] as const;
  const chosen = useQuery({ queryKey: currentKey(selected),
    // Native GET is non-null. A live cache withdrawal can remove a scoped row locally.
    queryFn: ({ signal }): Promise<Provider | null> => ok(api.GET('/providers/{id}', { params: { path: { id: selected } }, signal })),
    enabled: selected !== '' && !disabled, retry: false, staleTime: Infinity,
    refetchOnMount: false, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const currentError = chosen.error ?? (chosen.isFetching && currentRecovery?.id === selected ? currentRecovery.error : null);
  const current = currentError === null && chosen.isSuccess && chosen.data !== null && chosen.data.id === selected && holds(chosen.data.access, 'provider.view') ? chosen.data : undefined;
  const currentMissing = selected !== '' && chosen.isSuccess && current === undefined && currentError === null;
  const managed = currentMissing || currentError instanceof ApiError && [403, 404].includes(currentError.status);
  const listError = list.error ?? (list.isFetching && listRecovery?.query === settled ? listRecovery.error : null);
  const nextPageError = list.isFetchNextPageError || list.isFetching && listRecovery?.query === settled && listRecovery.more && listError !== null;
  const unverified = refused(listError) || list.isFetching && !list.isSuccess && list.errorUpdatedAt > 0;
  const rows = waiting || unverified || listError !== null && !nextPageError ? [] : [...new Map((list.data?.pages.flatMap((page) => page.items) ?? []).flatMap((item) => {
    const provider = item.resource?.kind === 'provider' ? item.resource.value : undefined;
    if (provider === undefined) return [];
    return item.kind === 'provider' && item.my_access !== 'path_only' && item.name !== null && item.id === provider.id
      && provider.status !== 'disabled' && holds(provider.access, 'provider.view') && holds(provider.access, 'provider.use')
      && !(provider.id === selected && (currentError !== null || currentMissing)) ? [[provider.id, provider] as const] : [];
  })).values()];
  const labels = providerLabels([...new Map([...(current === undefined ? [] : [current]), ...rows].map((row) => [row.id, row])).values()], known);
  const shown = selected === '' ? '' : managed ? t('settings.compact.managed') : current === undefined
    ? t(chosen.isPending && currentError === null ? 'places.providerDefault.loading' : 'places.providerDefault.unavailable')
    : labels.get(selected) ?? current.name;
  const busy = waiting || list.isFetching;
  const options: ComboboxOption<string>[] = [
    ...(text === '' && selected !== '' && !rows.some((row) => row.id === selected) ? [{ value: selected, label: shown, text: shown, disabled: true }] : []),
    ...rows.map((provider) => { const name = labels.get(provider.id) ?? provider.name; return { value: provider.id, ...choiceOption(t, 'dynamic.resource', { name }), ...resourceChoice({ kind: 'provider', provider }), text: name }; }),
  ];
  const changedOpen = (next: boolean) => {
    // SettingRow restores input focus in its layout effect after save/Undo. That
    // restoration is not a new request to browse, nor is a closed-name retry.
    if (next && (!mounted.current || restoring.current)) return;
    show(next); if (next) setVisited(true); else dismissed.current += 1;
  };
  const press = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!(event.target instanceof HTMLInputElement) || event.target.getAttribute('role') !== 'combobox') return;
    if (event.key === 'Escape' && open) type('');
    if (event.key !== 'Enter') return;
    if (isComposing(event.nativeEvent)) event.stopPropagation(); else event.preventDefault();
  };
  const removedAction = (trigger: Element | null) => document.activeElement === document.body && trigger instanceof HTMLElement && !trigger.isConnected;
  const ownsFocus = (trigger: Element | null) => frame.current !== null && (frame.current.contains(document.activeElement) || (footer.current?.contains(document.activeElement) ?? false) || removedAction(trigger));
  const focusInput = (browse: boolean) => {
    if (!browse) restoring.current = true;
    document.getElementById(session)?.focus();
    restoring.current = false;
  };
  const recover = (more: boolean) => {
    if (disabled || waiting || list.isFetching || more && !list.hasNextPage) return;
    const query = settled, trigger = document.activeElement, dismissal = dismissed.current;
    setListRecovery({ query, error: listError, more });
    const read = more ? list.fetchNextPage({ cancelRefetch: false }) : list.refetch({ cancelRefetch: false });
    void read.then((reply) => {
      if (!mounted.current || readerScope() !== reader || requested.current !== query || dismissed.current !== dismissal || !ownsFocus(trigger)) return;
      if (reply.isSuccess) { show(true); focusInput(true); }
      // A refusal can replace its pager after this promise settles. Restore
      // focus while the action still owns it, before React removes the button.
      else focusInput(visible.current);
    });
  };
  const retryCurrent = () => {
    if (selected === '' || disabled || chosen.isFetching) return;
    const id = selected, trigger = document.activeElement;
    setCurrentRecovery({ id, error: currentError });
    void chosen.refetch({ cancelRefetch: false }).then((reply) => {
      if (reply.isSuccess && mounted.current && readerScope() === reader && currentValue.current === id && ownsFocus(trigger)) focusInput(visible.current);
    });
  };
  const currentProblem = managed ? null : selected !== '' && currentError !== null ? <ErrorState compact title={t('places.providerDefault.unavailable')}
    error={currentError} retrying={chosen.isFetching} onRetry={disabled ? undefined : retryCurrent} />
    : null;
  const pages = !visited || waiting ? null : listError !== null && (!nextPageError || refused(listError))
    ? <ErrorState compact error={listError} retrying={list.isFetching} onRetry={disabled ? undefined : () => recover(false)} />
    : !disabled && <Pagination hasMore={list.hasNextPage} loading={list.isFetching} error={nextPageError ? listError : undefined} onMore={() => recover(true)} />;
  const hasFooter = currentProblem !== null || visited && !waiting && (listError !== null || list.hasNextPage);
  const empty = visited && text === '' && !waiting && list.isSuccess && listError === null && !list.hasNextPage
    && list.data.pages.every((page) => page.items.length === 0);
  const created = () => {
    if (!mounted.current || readerScope() !== reader) return;
    show(false);
    // A creation DTO does not establish subscription-sharing admission. Offer
    // only the next native usable answer, then wait for an explicit selection.
    void list.refetch({ cancelRefetch: false });
  };
  return <div ref={frame} className="flex min-w-0 flex-col gap-2" data-provider-default onKeyDownCapture={press}>
    <Combobox id={session} size="sm" label={label} hideLabel={showLabel !== true} disabled={disabled} error={error} fieldClassName="min-w-0"
      hint={current === undefined ? undefined : <ResourceName subject={{ kind: 'provider', provider: current }} hint={false}><ResourceHint subject={{ kind: 'provider', provider: current }} /></ResourceName>}
      query={typed} onQueryChange={type} displayValue={shown} initialActiveValue={selected} placeholder={placeholder ?? t('places.providerDefault.choose')}
      open={open} onOpenChange={changedOpen} loading={busy} empty={listError !== null ? undefined : t('ui.combobox.empty')}
      groups={[{ id: 'providers', label: '', options }]}
      footer={hasFooter ? <div ref={footer} className="flex min-w-0 flex-col gap-1">{currentProblem}{pages}</div> : undefined}
      onSelect={(option) => {
        if (disabled || busy) return;
        const provider = rows.find((row) => row.id === option.value);
        if (provider === undefined) return;
        cache.setQueryData(currentKey(provider.id), provider);
        show(false); type(''); setVisited(false); setListRecovery(null); setCurrentRecovery(null);
        if (provider.id !== selected) { currentValue.current = provider.id; onEdited?.(); onChange(provider.id); }
      }} />
    {!open && <>{currentProblem}{pages}</>}
    {empty && <div className="flex flex-wrap items-center gap-2">
      <span className="text-sm text-text-muted">{t('places.providerDefault.empty')}</span>
      {/* The existing dialog lives outside the popup, so input blur cannot unmount its action. */}
      {!disabled && <ResourceCreateButton kind="providers" primary={false} onCreated={created} />}
    </div>}
  </div>;
}
