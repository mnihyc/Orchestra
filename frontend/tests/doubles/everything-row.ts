import { resourceListRow } from './resource-rows.ts';
/** Complete inventory rows for deliberate browser and component scenarios, without loading the fake server. */
import type { EverythingRow } from '../../src/generated/api-types.ts';
import { MADE_AT } from './fixtures.ts';

export function inventoryRow(row: Pick<EverythingRow, 'id' | 'kind' | 'name'> & Partial<EverythingRow>): EverythingRow {
  return { home_path: [], participating: false, group: null, place_label: null, parent_id: null, has_children: false, path: [], created_at: MADE_AT,
    created_by: null, owner: null, last_activity: null, state: null, size_bytes: null, takes: null,
    my_access: 'view', tags: [], href: null, can_share: false, settings_href: null, ...row };
}

/** A table response carries its distinct row and never also embeds the resource's full page. */
export function resourceInventoryRow(row: Pick<EverythingRow, 'id' | 'kind' | 'name'> & Partial<EverythingRow>): EverythingRow {
  const { resource, ...metadata } = inventoryRow(row);
  return { ...metadata, ...((resource === null || resource === undefined) ? {} : { resource_row: resourceListRow(resource) }) };
}
