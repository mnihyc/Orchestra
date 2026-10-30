/** An explicit reveal request opens Navigation once without navigating the main page. */
import { create } from 'zustand';
import { client } from '@/api/client';
import type { EverythingRow } from '@/generated/api-types';
import { selectOrg } from '@/shell/org';
import { open } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';

export const useTreeReveal = create<{ row: EverythingRow | null }>(() => ({ row: null }));

export function showInTree(row: EverythingRow): void {
  const root = row.path[0]?.id ?? (row.kind === 'org' ? row.id : null);
  if (root === null) return;
  selectOrg(client.session.getState().userId, root);
  useTreeReveal.setState({ row });
  useWorkbench.getState().change((layout) => open(layout, 'navigation', 'left'));
}
