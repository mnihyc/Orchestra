/** Where a resource screen lies: Resources, then its kind, then itself; and the filter each list was last left with, so that the way back returns to it.
 *
 * A list says where it lies by its section; a resource's own screen by the section and its kind, each a
 * link, so that the first of them is the way back to the list it was reached from, with what was typed
 * and chosen there still in it. The row that goes from one kind to another is not a part of this path: it
 * belongs to the lists.
 */
import { client } from '@/api/client';
import type { NameKind } from '@/generated/api-types';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { useTranslation } from 'react-i18next';

import { type ScreenHeading, type TrailPart, useScreenHeading } from '@/shell/screen';

/** The kinds of resource that have a list of their own. */
export type ResourceKind = 'providers' | 'agents' | 'machines' | 'environments' | 'templates' | 'hosts' | 'mcp' | 'mounts' | 'skills';

const TARGET_KINDS: Record<ResourceKind, NameKind> = { providers:'provider', agents:'agent', machines:'machine', environments:'environment', templates:'template', hosts:'host', mcp:'mcp_server', mounts:'mount', skills:'skill' };

/** The section itself opens the landing, leaving each list's own filters intact. */
const SECTION_TO = '/resources';

/** What each list was last left with, by kind: the words typed and the filter chosen, as its address says them. */
const left = new Map<ResourceKind, Readonly<Record<string, string>>>();

/** Keeps what a list's address says, so that a screen reached from the list can lead back to it as it was. */
export function rememberList(kind: ResourceKind, search: Readonly<Record<string, string | undefined>>): void {
  const kept: Record<string, string> = {};
  for (const [name, value] of Object.entries(search)) if (value !== undefined && value !== '') kept[name] = value;
  left.set(kind, kept);
  writeViewChoice(client.session.getState().userId, `resource-${kind}`, 'listOrigin', JSON.stringify(kept));
}

/** What the list of a kind was last left with: nothing, for one that was never opened. */
export function listAsLeft(kind: ResourceKind): Readonly<Record<string, string>> | undefined {
  const stored = readViewChoice<string>(client.session.getState().userId, `resource-${kind}`, 'listOrigin', '');
  let search = left.get(kind);
  if (stored !== '') {
    try {
      const parsed: unknown = JSON.parse(stored);
      if (typeof parsed === 'object' && parsed !== null && Object.values(parsed).every((value) => typeof value === 'string')) search = parsed as Record<string, string>;
    } catch { /* An old unreadable origin falls back to the normal list. */ }
  }
  return search === undefined || Object.keys(search).length === 0 ? undefined : search;
}

/** The parts of the way to a screen of a kind, outermost first: the section, and the kind's list as it was last left. */
export function useResourceTrail(kind: ResourceKind): { section: TrailPart; list: TrailPart } {
  const { t } = useTranslation();
  const search = listAsLeft(kind);
  return {
    section: { label: t('screens.sections.resources'), to: SECTION_TO },
    list: { label: t(`screens.${kind}.title`), to: `/resources/${kind}`, ...(search === undefined ? {} : { search }) },
  };
}

/**
 * Says what a resource screen is, for the title bar: a list under the section, and a resource under its
 * kind. Gives back the way to the kind's list, which the heading of the screen draws as an arrow at
 * widths where the path above it has no room.
 */
export function useResourceHeading(kind: ResourceKind, icon: ScreenHeading['icon'], name?: string, targetId?: string, enabled = true): TrailPart | undefined {
  const { t } = useTranslation();
  const { section, list } = useResourceTrail(kind);
  const own = name === undefined;
  useScreenHeading({
    target: targetId === undefined ? undefined : { kind: TARGET_KINDS[kind], id: targetId },
    title: name ?? t(`screens.${kind}.title`),
    icon,
    section: t('screens.sections.resources'),
    trail: own ? [section] : [section, list],
  }, enabled);
  return own ? undefined : list;
}
