/** Explicit normalized artifact facts exercise the ordinary Storage rendering, not a native harness. */
import type { KeptRecord, StorageQuery, StorageReport } from '@/generated/api-types';
import { storageReport } from './storage-report';

export function artifactStorageReport(query: StorageQuery = {}): StorageReport {
  const original = storageReport();
  const template = original.items[0]!;
  const base: KeptRecord = { ...template, resource: null, host: null, machine: null, place: null,
    why: 'restore', until: null, facts: null, can_clean: false, blocker: null };
  const records: KeptRecord[] = [
    { ...base, id: 'snapshot:one', kind: 'snapshot', name: 'Workspace snapshot', bytes: 256, accounting_key: 'snapshot:one',
      physical_parts: [{ key: 'blob:shared', bytes: 256 }], artifact: { id: '00000000-0000-4000-8000-000000000701', kind: 'snapshot', state: 'complete' } },
    { ...base, id: 'export:one', kind: 'export', name: 'Export package', bytes: 512, accounting_key: 'blob:archive',
      physical_parts: [{ key: 'blob:archive', bytes: 512 }], artifact: { id: '00000000-0000-4000-8000-000000000702', kind: 'export', state: 'ready' } },
    { ...base, id: 'import:one', kind: 'import', name: 'reviewed.md', bytes: 256, accounting_key: 'import:one',
      physical_parts: [{ key: 'blob:shared', bytes: 256 }], artifact: { id: '00000000-0000-4000-8000-000000000703', kind: 'import', state: 'pending' } },
  ];
  const items = records.filter(row => (query.kind == null || row.kind === query.kind) && (query.owner == null || row.owner?.id === query.owner));
  // These fixture facts have one shared 256-byte source and one distinct 512-byte package.
  const bytes = (items.some(row => row.kind === 'export') ? 512 : 0) + (items.some(row => row.kind !== 'export') ? 256 : 0);
  return { ...original, scope: 'system', items, totals: { items: items.length, bytes, unknown: 0 }, breakdowns: [], unavailable: [] };
}
