/** Cards describe the same audience and current management authority as the short access form. */
import type { AccessSummary, PermissionPage } from '@/generated/api-types';
import { containerOf, followedPlace, linesOf, type Catalogue } from './model';

export function cardSummary(page: PermissionPage, catalogue: Catalogue, kind: string): AccessSummary | null {
  if (!page.owners_writable && !page.rows.some((row) => row.writable)) return null;
  const lines = linesOf(page, catalogue, kind);
  return {
    manage: lines.manage.map((line) => line.holder),
    interact: lines.interact.map((line) => line.holder),
    view: lines.view.map((line) => line.holder),
    custom: lines.custom.length !== 0,
    from: followedPlace(page.rows) ?? containerOf(page),
  };
}
