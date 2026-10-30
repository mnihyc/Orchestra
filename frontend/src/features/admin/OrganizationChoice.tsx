/** The IDP's default org: native administrator name search/pages, with an independent current identity. */
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { type KeyboardEvent, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { adminOrgsQuery } from '@/api/admin';
import { ApiError, api, client, ok } from '@/api/client';
import { nodeKey } from '@/api/nodes';
import { choiceOption } from '@/i18n/choices';
import { useDebounced } from '@/lib/debounce';
import { isComposing } from '@/lib/shortcuts';
import { Combobox, type ComboboxOption } from '@/ui/Combobox';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Pagination } from '@/ui/Pagination';

interface Props {
  value: string | null;
  onChange: (id: string | null) => void;
  disabled?: boolean;
}
const NONE = '\u0000none';
const denied = (error: unknown) => error instanceof ApiError && [401, 403, 404].includes(error.status);

export function OrganizationChoice(props: Props) {
  const reader = useStore(client.session, (state) => state.readerVersion ?? 0);
  // Authority changes end the old query/focus session, without replacing the form's draft ID.
  return <ReaderChoice key={reader} {...props} reader={reader} />;
}

function ReaderChoice({ value, onChange, disabled = false, reader }: Props & { reader: number }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const session = useId();
  const frame = useRef<HTMLDivElement>(null);
  const footer = useRef<HTMLDivElement>(null);
  const currentValue = useRef(value);
  useEffect(() => { currentValue.current = value; }, [value]);
  const [open, setOpen] = useState(false), [visited, setVisited] = useState(false), [typed, setTyped] = useState('');
  const [listRecovery, setListRecovery] = useState<{ query: string; error: unknown; more: boolean } | null>(null);
  const [currentRecovery, setCurrentRecovery] = useState<{ id: string; error: unknown } | null>(null);
  const text = typed.trim(), settled = useDebounced(text), waiting = text !== settled;
  const requested = useRef('');
  const type = (next: string) => { requested.current = next.trim(); setTyped(next); };
  const source = adminOrgsQuery({ q: settled, deleted: false }, { reader, field: session });
  // Freshness belongs to this mounted form session. Returning focus from its
  // pager never refetches; a newly opened form makes its own authorized reads.
  const orgs = useInfiniteQuery({ ...source,
    enabled: open && !disabled && !waiting, retry: false, staleTime: Infinity,
    refetchOnMount: false, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const currentKey = (id: string) => [...nodeKey(id), 'idp-choice', reader, session] as const;
  const current = useQuery({ queryKey: currentKey(value ?? ''),
    queryFn: ({ signal }) => ok(api.GET('/nodes/{id}', { params: { path: { id: value ?? '' } }, signal })),
    enabled: value !== null && !disabled, retry: false, staleTime: Infinity,
    refetchOnMount: false, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const currentError = current.error ?? (current.isFetching && currentRecovery?.id === value ? currentRecovery.error : null);
  const listError = orgs.error ?? (orgs.isFetching && listRecovery?.query === settled ? listRecovery.error : null);
  const nextPageError = orgs.isFetchNextPageError || orgs.isFetching && listRecovery?.query === settled && listRecovery.more && listError !== null;
  const selected = current.isSuccess && current.data.kind === 'org' && current.data.id === value ? current.data : undefined;
  const shown = value === null ? t('admin.idp.noOrg') : selected?.name
    ?? t(current.isPending && currentError === null ? 'common.loading' : 'settings.resourceChoice.unavailable');
  // A failed authorization read cannot lend its old rows to a retry. A transient
  // next-page error may retain legitimate rows until that retry begins.
  const unverified = denied(listError) || orgs.isFetching && !orgs.isSuccess && orgs.errorUpdatedAt > 0;
  const rows = waiting || unverified || listError !== null && !nextPageError ? []
    : [...new Map((orgs.data?.pages.flatMap((page) => page.items) ?? []).map((org) => [org.id, org])).values()];
  const busy = waiting || orgs.isFetching;
  const options: ComboboxOption<string>[] = [
    ...(text === '' ? [{ value: NONE, label: t('admin.idp.noOrg'), text: t('admin.idp.noOrg') }] : []),
    ...(text === '' && value !== null && !rows.some((org) => org.id === value)
      ? [{ value, label: shown, text: shown, disabled: true }] : []),
    ...rows.map((org) => {
      const copy = choiceOption(t, 'dynamic.place', { name: org.name });
      return { value: org.id, ...copy, text: org.name, description: [copy.description, org.address].filter(Boolean).join(' · ') };
    }),
  ];
  const choose = (id: string | null) => { currentValue.current = id; onChange(id); type(''); setOpen(false); setVisited(false); setListRecovery(null); setCurrentRecovery(null); };
  const changedOpen = (next: boolean) => { setOpen(next); if (next) setVisited(true); };
  const press = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!(event.target instanceof HTMLInputElement) || event.target.getAttribute('role') !== 'combobox') return;
    if (event.key === 'Escape' && open) type('');
    if (event.key !== 'Enter') return;
    if (isComposing(event.nativeEvent)) event.stopPropagation(); else event.preventDefault();
  };
  const recover = (more: boolean) => {
    if (disabled || waiting || orgs.isFetching || more && !orgs.hasNextPage) return;
    const query = settled;
    const trigger = document.activeElement;
    setListRecovery({ query, error: listError, more });
    // The button remains mounted after input blur. Reopen only after this one
    // explicit read succeeds and the person is still in this field's controls.
    const read = more ? orgs.fetchNextPage({ cancelRefetch: false }) : orgs.refetch({ cancelRefetch: false });
    void read.then((reply) => {
      // A final page or successful retry may remove its own button. That loss
      // of focus belongs to this action; moving to another field does not.
      const removedAction = document.activeElement === document.body && trigger instanceof HTMLElement && !trigger.isConnected;
      if (reply.isSuccess && requested.current === query && frame.current !== null
        && (frame.current.contains(document.activeElement) || footer.current?.contains(document.activeElement) || removedAction)) {
        setOpen(true); document.getElementById(session)?.focus();
      }
    });
  };
  const retryCurrent = () => {
    if (value === null || disabled || current.isFetching) return;
    const id = value, trigger = document.activeElement;
    setCurrentRecovery({ id: value, error: currentError });
    void current.refetch({ cancelRefetch: false }).then((reply) => {
      const removedAction = document.activeElement === document.body && trigger instanceof HTMLElement && !trigger.isConnected;
      if (reply.isSuccess && currentValue.current === id && frame.current !== null
        && (frame.current.contains(document.activeElement) || footer.current?.contains(document.activeElement) || removedAction)) {
        // Recovering a closed current-name read must not open/fetch the candidate list.
        (footer.current ?? frame.current).querySelector<HTMLButtonElement>('[data-org-clear]')?.focus();
      }
    });
  };
  const clear = () => value === null ? null : <IconButton size="sm" label={t('resources.bulk.clear')}
    data-org-clear icon={<X aria-hidden />} disabled={disabled} onClick={() => {
      document.getElementById(session)?.focus(); choose(null);
    }} />;
  const currentProblem = value !== null && currentError !== null ? <ErrorState compact error={currentError}
    retrying={current.isFetching} onRetry={disabled ? undefined : retryCurrent} /> : null;
  const pages = !visited || waiting ? null : listError !== null && (!nextPageError || denied(listError))
    ? <ErrorState compact error={listError} retrying={orgs.isFetching} onRetry={disabled ? undefined : () => recover(false)} />
    : !disabled && <Pagination hasMore={orgs.hasNextPage} loading={orgs.isFetching}
      error={nextPageError ? listError : undefined} onMore={() => recover(true)} />;
  const hasFooter = value !== null || currentProblem !== null || visited && !waiting && (listError !== null || orgs.hasNextPage);
  return <div ref={frame} className="flex min-w-0 flex-col gap-1" data-admin-org-choice onKeyDownCapture={press}>
      <Combobox label={t('admin.idp.defaultOrg')} hint={t('admin.idp.defaultOrgHint')} placeholder={t('ui.combobox.search')}
        id={session}
        trailing={value === null || open ? undefined : clear()}
        query={typed} onQueryChange={type} displayValue={shown} initialActiveValue={value ?? NONE}
        open={open} onOpenChange={changedOpen} disabled={disabled} loading={busy}
        fieldClassName="min-w-0" empty={listError !== null ? undefined : t('ui.combobox.empty')}
        groups={[{ id: 'organizations', label: '', options }]}
        footer={hasFooter ? <div ref={footer} className="flex min-w-0 flex-col gap-1">
          {value !== null && <div className="flex justify-end">{clear()}</div>}{currentProblem}{pages}
        </div> : undefined}
        onSelect={(option) => {
          if (disabled || busy) return;
          if (option.value === NONE) { choose(null); return; }
          const org = rows.find((row) => row.id === option.value);
          if (org === undefined) return;
          cache.setQueryData(currentKey(org.id), org);
          choose(org.id);
        }} />
    {!open && <>{currentProblem}{pages}</>}
  </div>;
}
