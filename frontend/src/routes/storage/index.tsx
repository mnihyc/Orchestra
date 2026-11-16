/** One storage report restores categorical filters on this device and carries them in its address. */
import { createFileRoute } from '@tanstack/react-router';
import { useEffect, useMemo, useState } from 'react';
import { usePageNavigate, usePageSearch } from '@/features/workbench/pageContext';
import { OwnUploads } from '@/features/uploads/Inventory';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { Button } from '@/ui/Button';
import { useTranslation } from 'react-i18next';
import { Storage } from '@/features/storage/Storage';
import { Cleanup } from '@/features/storage/Cleanup';
import { patchStorageSearch, readStorageSearch, type StorageSearch } from '@/features/storage/search';
import { useViewChoice } from '@/lib/viewChoices';
import { bindDeviceChoice } from '@/lib/viewChoiceFacts';

export const Route = createFileRoute('/storage/')({
  staticData: { screen: 'page' },
  validateSearch: readStorageSearch,
  component: Screen,
});

function savedSearch(value: string): { value: StorageSearch; unavailable: boolean } {
  try {
    const parsed: unknown = JSON.parse(value);
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return { value: {}, unavailable: true };
    const current = readStorageSearch(parsed as Record<string, unknown>);
    return { value: current, unavailable: Object.entries(parsed).some(([key, value]) => JSON.stringify(Reflect.get(current, key)) !== JSON.stringify(value)) };
  } catch { return { value: {}, unavailable: true }; }
}

function Screen() {
  const { t } = useTranslation();
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  const [kept, remember, receipt] = useViewChoice<string>('storage', 'filters', '{}');
  const saved = useMemo(() => savedSearch(kept), [kept]), restored = saved.value;
  const [ignoreSaved, setIgnoreSaved] = useState(false);
  const [unavailable, setUnavailable] = useState(saved.unavailable);
  const [selection, setSelection] = useState<string[] | null>(null);
  const [wasRestored, setWasRestored] = useState(() => Object.keys(search).length === 0 && Object.keys(restored).length > 0);
  const shown = !ignoreSaved && Object.keys(search).length === 0 ? restored : search;
  const observation = receipt === undefined ? undefined : { ...receipt, effective: shown, origin: unavailable ? 'default' as const : wasRestored ? receipt.origin : 'address' as const,
    saved_choice_availability: unavailable ? 'unavailable' as const : receipt.saved_choice_availability };
  useEffect(() => {
    if (!ignoreSaved && Object.keys(search).length === 0 && Object.keys(restored).length > 0) void navigate({ search: restored, replace: true });
  }, [ignoreSaved, navigate, restored, search]);
  if (shown.view === 'uploads') return <PersonalFrame page="uploads" lead={null}><div className="flex min-w-0 flex-col gap-4"><div><Button size="sm" onClick={() => { const report: StorageSearch = { ...restored, view: 'report' }; setIgnoreSaved(true); void navigate({ search: report, replace: true }); }}>{t('storage.title')}</Button></div><OwnUploads /></div></PersonalFrame>;
  return <><Storage search={shown} onCleanup={setSelection} onReportReady={() => {
    if (unavailable) return;
    const stored = {...shown};
    delete stored.cursor;
    const value=JSON.stringify(stored);
    if(value!==kept) remember(value);
  }} onScopeUnavailable={() => {
    if (!wasRestored) return;
    setWasRestored(false);
    setUnavailable(true);
    setIgnoreSaved(true);
    void navigate({ search: {}, replace: true });
  }} onSearch={bindDeviceChoice((patch: Parameters<typeof patchStorageSearch>[1]) => {
    const next = patchStorageSearch(shown, patch);
    const stored = { ...next };
    delete stored.cursor;
    remember(JSON.stringify(stored));
    setUnavailable(false);
    setWasRestored(false);
    setIgnoreSaved(true);
    void navigate({ search: next, replace: true });
  }, observation)} />{selection !== null && <Cleanup request={{ scope: shown.scope ?? 'me', resource: shown.resource_id !== undefined && shown.resource_kind !== undefined ? {kind:shown.resource_kind,id:shown.resource_id} : null, items: selection }} onClose={() => setSelection(null)} />}</>;
}
