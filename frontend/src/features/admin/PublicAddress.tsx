/** Compact public-address fields retain a failed or newer draft while the edge is prepared. */
import { useQuery } from '@tanstack/react-query';
import { Plus, Trash2 } from 'lucide-react';
import { useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { metaQuery } from '@/api/auth';
import { ApiError } from '@/api/client';
import { publicAddressQuery, updatePublicAddress } from '@/api/publicAddress';
import { RESOURCE_PAGE_SIZE } from '@/api/resources';
import { useHoldDraft } from '@/features/settings/drafts';
import type { PublicAddressSnapshot, PublicAddressUpdateBody, PublicAddressUpdated } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';

interface Row { key: number; value: string }
interface Draft { primary: string; alternates: Row[] }
interface Props { snapshot: PublicAddressSnapshot; onSave: (body: PublicAddressUpdateBody) => Promise<PublicAddressUpdated>;
  onOpen?: (origin: string) => void; onRefresh?: () => Promise<PublicAddressSnapshot>; writable?: boolean }
const openOrigin = (origin: string) => window.location.assign(origin);
const values = (draft: Draft) => draft.alternates.map((row) => row.value.trim()).filter(Boolean);
const same = (draft: Draft, snapshot: Pick<PublicAddressSnapshot, 'public_url' | 'alternate_urls'>) => draft.primary.trim() === snapshot.public_url && JSON.stringify(values(draft)) === JSON.stringify(snapshot.alternate_urls);
const initial = (snapshot: PublicAddressSnapshot): Draft => ({ primary: snapshot.public_url, alternates: snapshot.alternate_urls.map((value, key) => ({ key, value })) });
function origin(value: string): boolean {
  try { const url = new URL(value); return value.trim() === value && !value.includes('\\') && !value.includes('%') && ['http:', 'https:'].includes(url.protocol) && url.username === '' && url.password === '' && url.pathname === '/' && url.search === '' && url.hash === ''; } catch { return false; }
}

export function PublicAddressEditor({ snapshot, onSave, onOpen = openOrigin, onRefresh, writable = true }: Props) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [accepted, setApplied] = useState(snapshot);
  const applied = snapshot.revision > accepted.revision ? snapshot : accepted;
  const latest = useRef(applied);
  useLayoutEffect(() => { latest.current = applied; }, [applied]);
  const [baseRevision, setBaseRevision] = useState(snapshot.revision);
  const [draft, setDraft] = useState(() => initial(snapshot));
  const draftRef = useRef(draft), base = useRef(snapshot.revision), epoch = useRef(0), next = useRef(snapshot.alternate_urls.length);
  const [pending, setPending] = useState(false), busy = useRef(false), alive = useRef(true);
  const [error, setError] = useState<unknown>(null), [invalid, setInvalid] = useState(false);
  const [search, setSearch] = useState(''), deferredSearch = useDeferredValue(search), [limit, setLimit] = useState(RESOURCE_PAGE_SIZE);
  const [active, setActive] = useState<number | null>(null), [navigation, setNavigation] = useState<{ url: string; revision: number } | null>(null);
  const opened = useRef<number | null>(null);
  const dirty = !same(draft, applied);
  if (snapshot.revision > accepted.revision) {
    setApplied(snapshot);
    if (!pending && (same(draft, accepted) || same(draft, snapshot))) { setDraft(initial(snapshot)); setBaseRevision(snapshot.revision); }
  }
  useEffect(() => { draftRef.current = draft; for (const row of draft.alternates) next.current = Math.max(next.current, row.key + 1); }, [draft]);
  useEffect(() => { base.current = baseRevision; }, [baseRevision]);
  const mounted = useCallback(() => alive.current, []);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const edit = (change: (previous: Draft) => Draft) => { const value = change(draftRef.current); draftRef.current = value; epoch.current += 1; setDraft(value); setInvalid(false); setError(null); setNavigation(null); };
  const adopt = useCallback((value: PublicAddressSnapshot) => {
    const valueDraft = initial(value); next.current = value.alternate_urls.length; draftRef.current = valueDraft;
    base.current = value.revision; setBaseRevision(value.revision); setDraft(valueDraft); setInvalid(false);
  }, []);
  const discard = useCallback(() => { adopt(applied); setError(null); }, [adopt, applied]);
  const save = useCallback(async (open: boolean): Promise<boolean> => {
    if (busy.current || !writable) return false;
    const submitted = draftRef.current, at = epoch.current;
    const body: PublicAddressUpdateBody = { expected_revision: base.current, public_url: submitted.primary.trim(), alternate_urls: values(submitted) };
    if (!origin(body.public_url) || body.alternate_urls.some((value) => !origin(value))) { setInvalid(true); return false; }
    busy.current = true; setPending(true); setError(null);
    try {
      const receipt = await onSave(body);
      if (!mounted()) return true;
      const saved: PublicAddressSnapshot = { public_url: receipt.public_url, alternate_urls: receipt.alternate_urls, revision: receipt.revision, using_example: receipt.using_example, from_file_seed: false };
      const current = latest.current.revision > saved.revision ? latest.current : saved;
      setApplied(current); base.current = current.revision; setBaseRevision(current.revision);
      if (epoch.current === at) {
        adopt(current);
        if (open && current.revision === saved.revision) setNavigation({ url: receipt.public_url, revision: receipt.revision });
      }
      return epoch.current === at;
    } catch (failure) {
      if (!mounted()) return false;
      setError(failure);
      if (failure instanceof ApiError && failure.problem.code === 'conflict.stale_revision' && onRefresh !== undefined) {
        try {
          const refreshed = await onRefresh();
          if (!mounted()) return false;
          const current = latest.current.revision > refreshed.revision ? latest.current : refreshed;
          setApplied(current); base.current = current.revision; setBaseRevision(current.revision);
          if (same(submitted, current) && epoch.current === at) {
            adopt(current); setError(null);
            if (open) setNavigation({ url: current.public_url, revision: current.revision });
            return true;
          }
        } catch { /* Keep the same draft and its refusal if its current projection cannot be read. */ }
      }
      return false;
    } finally { busy.current = false; if (mounted()) setPending(false); }
  }, [adopt, mounted, onRefresh, onSave, writable]);
  useHoldDraft(dirty, { save: () => save(false), discard });
  useEffect(() => {
    if (navigation?.revision !== applied.revision || dirty || pending || opened.current === navigation.revision) return;
    opened.current = navigation.revision; onOpen(navigation.url);
  }, [navigation, applied.revision, dirty, pending, onOpen]);
  const filtered = useMemo(() => draft.alternates.map((row, index) => ({ row, index })).filter(({ row }) => row.key === active || row.value.toLocaleLowerCase().includes(deferredSearch.toLocaleLowerCase())), [draft.alternates, active, deferredSearch]);
  const shown = filtered.slice(0, limit);
  const add = () => { setSearch(''); setLimit(RESOURCE_PAGE_SIZE); edit((old) => ({ ...old, alternates: [{ key: next.current++, value: '' }, ...old.alternates] })); };
  return <div className="flex min-w-0 flex-col gap-3" data-public-address-editor>
    <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-text-muted">
      <span>{t('publicAddress.appliedRevision', { revision: formatNumber(applied.revision, locale) })}</span>
      <Button size="sm" variant="ghost" onClick={() => onOpen(applied.public_url)}>{t('common.open')}</Button>
    </div>
    {applied.using_example && <InlineAlert tone="warning">{t('publicAddress.example')}</InlineAlert>}
    <Input label={t('publicAddress.primary')} value={draft.primary} readOnly={!writable} autoComplete="off" autoCapitalize="none" spellCheck={false} inputMode="url"
      help={<HelpHint term={t('publicAddress.title')} what={t('publicAddress.help')} guide={{ article: 'public-address', heading: 'change-address' }} />}
      error={invalid && !origin(draft.primary.trim()) ? t('publicAddress.invalid') : undefined} onChange={(event) => edit((old) => ({ ...old, primary: event.target.value }))} />
    <fieldset className="flex min-w-0 flex-col gap-2">
      <legend className="flex w-full items-center justify-between gap-2 text-sm font-medium text-text"><span>{t('publicAddress.alternates')}</span>
        <IconButton size="sm" icon={<Plus aria-hidden />} label={t('publicAddress.add')} disabled={!writable} onClick={add} /></legend>
      {draft.alternates.length > 10 && <Input type="search" label={t('publicAddress.search')} hideLabel value={search} onChange={(event) => { setSearch(event.target.value); setLimit(RESOURCE_PAGE_SIZE); }} />}
      <div className="max-h-64 overflow-y-auto" role="group" aria-label={t('publicAddress.alternates')}>
        {shown.map(({ row, index }) => <div key={row.key} className="flex min-w-0 items-end gap-2 py-1">
          <Input label={t('publicAddress.alternateNumber', { number: formatNumber(index + 1, locale) })} hideLabel fieldClassName="min-w-0 flex-1" value={row.value} readOnly={!writable} inputMode="url" autoComplete="off" autoCapitalize="none" spellCheck={false}
            onFocus={() => setActive(row.key)} onBlur={() => setActive(null)}
            error={invalid && row.value.trim() !== '' && !origin(row.value.trim()) ? t('publicAddress.invalid') : undefined}
            onChange={(event) => edit((old) => ({ ...old, alternates: old.alternates.map((entry) => entry.key === row.key ? { ...entry, value: event.target.value } : entry) }))}
            onPaste={(event) => { const lines = event.clipboardData.getData('text').split(/\r?\n/).map((line) => line.trim()).filter(Boolean); if (lines.length < 2) return; event.preventDefault(); edit((old) => ({ ...old, alternates: old.alternates.flatMap((entry) => entry.key === row.key ? lines.map((value) => ({ key: next.current++, value })) : [entry]) })); }} />
          <IconButton size="sm" icon={<Trash2 aria-hidden />} label={t('publicAddress.removeNumber', { number: formatNumber(index + 1, locale) })} disabled={!writable} onClick={() => edit((old) => ({ ...old, alternates: old.alternates.filter((entry) => entry.key !== row.key) }))} />
        </div>)}
        {draft.alternates.length === 0 && <p className="text-sm text-text-muted">{t('publicAddress.none')}</p>}
        {draft.alternates.length > 0 && filtered.length === 0 && <p className="text-sm text-text-muted">{t('publicAddress.noMatches')}</p>}
        <Pagination hasMore={filtered.length > shown.length} loading={false} onMore={() => setLimit((count) => count + RESOURCE_PAGE_SIZE)} />
      </div>
    </fieldset>
    {error !== null && <ErrorAlert error={error} />}
    <div className="flex flex-wrap items-center justify-end gap-2">
      {dirty && <Button size="sm" disabled={pending} onClick={discard}>{t('common.cancel')}</Button>}
      <Button variant="primary" pending={pending} disabled={!writable || !dirty} onClick={() => void save(true)}>{t(pending ? 'publicAddress.preparing' : 'publicAddress.saveOpen')}</Button>
    </div>
  </div>;
}

export function PublicAddressSettings() {
  const { t } = useTranslation();
  const query = useQuery(publicAddressQuery), meta = useQuery(metaQuery).data;
  const { isFetching, refetch } = query;
  const attempted = useRef(0);
  const revision = meta?.public_address_revision, appliedRevision = query.data?.revision;
  useEffect(() => {
    if (revision === undefined || appliedRevision === undefined || revision <= appliedRevision || attempted.current >= revision || isFetching) return;
    attempted.current = revision; void refetch();
  }, [revision, appliedRevision, isFetching, refetch]);
  const refresh = async () => { const result = await refetch(); if (result.data === undefined) throw result.error ?? new Error(t('publicAddress.loadFailed')); return result.data; };
  return <Section id="admin-public-address" title={t('publicAddress.title')}>
    {query.isPending ? <SkeletonRows read={query} rows={2} /> : query.data === undefined ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : <>
      {query.isError && <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />}
      <PublicAddressEditor snapshot={query.data} onSave={updatePublicAddress} onRefresh={refresh} writable={!query.isError} />
    </>}
  </Section>;
}
