/** One storage report restores categorical filters on this device and carries them in its address. */
import { createFileRoute } from '@tanstack/react-router';
import { useEffect, useMemo, useState } from 'react';
import { usePageNavigate, usePageSearch } from '@/features/workbench/pageContext';
import { Storage } from '@/features/storage/Storage';
import { Cleanup } from '@/features/storage/Cleanup';
import { patchStorageSearch, readStorageSearch, type StorageSearch } from '@/features/storage/search';
import { useViewChoice } from '@/lib/viewChoices';

export const Route = createFileRoute('/storage/')({
  staticData: { screen: 'page' },
  validateSearch: readStorageSearch,
  component: Screen,
});

function savedSearch(value: string): StorageSearch {
  try {
    const parsed: unknown = JSON.parse(value);
    return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed) ? readStorageSearch(parsed as Record<string, unknown>) : {};
  } catch { return {}; }
}

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  const [kept, remember] = useViewChoice<string>('storage', 'filters', '{}');
  const restored = useMemo(() => savedSearch(kept), [kept]);
  const [ignoreSaved, setIgnoreSaved] = useState(false);
  const [selection, setSelection] = useState<string[] | null>(null);
  const [wasRestored, setWasRestored] = useState(() => Object.keys(search).length === 0 && Object.keys(restored).length > 0);
  const shown = !ignoreSaved && Object.keys(search).length === 0 ? restored : search;
  useEffect(() => {
    if (!ignoreSaved && Object.keys(search).length === 0 && Object.keys(restored).length > 0) void navigate({ search: restored, replace: true });
  }, [ignoreSaved, navigate, restored, search]);
  return <><Storage search={shown} onCleanup={setSelection} onReportReady={() => {
    const stored = {...shown};
    delete stored.cursor;
    const value=JSON.stringify(stored);
    if(value!==kept) remember(value);
  }} onScopeUnavailable={() => {
    if (!wasRestored) return;
    setWasRestored(false);
    setIgnoreSaved(true);
    void navigate({ search: {}, replace: true });
  }} onSearch={(patch) => {
    const next = patchStorageSearch(shown, patch);
    const stored = { ...next };
    delete stored.cursor;
    remember(JSON.stringify(stored));
    setWasRestored(false);
    setIgnoreSaved(true);
    void navigate({ search: next, replace: true });
  }} />{selection !== null && <Cleanup request={{ scope: shown.scope ?? 'me', resource: shown.resource_id !== undefined && shown.resource_kind !== undefined ? {kind:shown.resource_kind,id:shown.resource_id} : null, items: selection }} onClose={() => setSelection(null)} />}</>;
}
