/** Letting people into the environment a thread runs in: what sharing a thread offers of it, what that gives and no more, and the change or the request that does it.
 *
 * A thread's files, terminal, browsers and displays are the environment's, given on its page. Sharing the thread
 * offers them in the same step: Can view, Can interact and Can manage of the thread offer the environment's Can
 * view (it and its four doors, to look); Can manage of the thread offers the Manage of Files and Terminal
 * as well. Never the environment's own Manage, and never the Manage of Browsers or VNC: those are given on the
 * environment's page, by name. Where the person at the page may give there, the people just named are named on the
 * environment's page in one change; where they may not, the owner of the environment is asked, for each person,
 * with the same entry.
 */
import type { AccessChoice, AccessRequestCreateBody, Dependency, HolderInput, PageChange, PermissionPage } from '@/generated/api-types';
import { changePage, readPage } from '@/api/access';

import { containerOf, holderKey, holdsOfRow, inputOf } from './model';

/** The offers of a thread's page that are about its environment, whichever the choice. */
export const environmentDependencies = (page: PermissionPage): Dependency[] => page.depends.filter((entry) => entry.object.kind === 'environment');

/** The permissions the environment's Can view names beside its own: it and the View row of each of its four doors. */
const VIEW_ROWS = ['environment.view', 'files.view', 'terminal.view', 'browser.view', 'vnc.view', 'ports.view'] as const;

/** What letting the people named into the environment comes to for one of the three choices. */
export interface Offer {
  environment: Dependency['object'];
  /** The rows of the environment's page that are given: View of it and of its doors, and with Can manage the Manage of Files and Terminal. */
  rows: string[];
  /** Whether the person at the page may give the View rows themself; where not, the owner is asked. */
  mayView: boolean;
  /** The Manage rows they may give, and those they may not. */
  giveManage: string[];
  askManage: string[];
}

/** What a choice of the thread offers of its environment, or `null` where the page offers nothing there. */
export function offerFor(page: PermissionPage, choice: AccessChoice): Offer | null {
  const found = environmentDependencies(page);
  const view = found.find((entry) => entry.for === 'thread.view');
  if (view === undefined) return null;
  const manage = choice === 'full_manage' ? found.filter((entry) => entry.for === 'thread.manage') : [];
  return {
    environment: view.object,
    rows: [...VIEW_ROWS, ...manage.map((entry) => entry.permission)],
    mayView: view.can_give,
    giveManage: manage.filter((entry) => entry.can_give).map((entry) => entry.permission),
    askManage: manage.filter((entry) => !entry.can_give).map((entry) => entry.permission),
  };
}

/** The change of the environment's page that names the people there: its Can view, and the Manage rows the person at the page may give. */
export function letInChange(revision: number, holders: readonly HolderInput[], offer: Offer, expiresAt: string | null): PageChange {
  const when = expiresAt === null ? {} : { expires_at: expiresAt };
  return {
    expected_revision: revision,
    ...(offer.mayView ? { name: [{ choice: 'full_view' as const, holders: [...holders], ...when }] } : {}),
    ...(offer.giveManage.length === 0
      ? {}
      : { rows: offer.giveManage.map((permission) => ({ permission, add: holders.map((holder) => ({ holder, ...when })) })) }),
  };
}

/** The audience which can actually view the confirmed thread page, with implicit roles pinned to its topic. */
export function threadViewers(page: PermissionPage): HolderInput[] {
  const place = containerOf(page);
  const row = page.rows.find((entry) => entry.permission === 'thread.view');
  if (row === undefined) return [];
  const holders = holdsOfRow(row).map(({ holder }) => holder.kind === 'role' && holder.at === null && place !== null && place !== 'system'
    ? { kind: 'role' as const, role: holder.role, at: place.id }
    : inputOf(holder));
  return [...new Map(holders.map((holder) => [holderKey(holder), holder])).values()];
}

/** Sharing a thread's general audience only lets its confirmed viewers look into its environment. */
export async function letThreadViewersIn(page: PermissionPage): Promise<boolean> {
  const offer = offerFor(page, 'full_view');
  const holders = threadViewers(page);
  if (offer === null || holders.length === 0) return true;
  if (!offer.mayView) return false;
  const target = { kind: 'environment' as const, id: offer.environment.id, name: offer.environment.name };
  const environment = await readPage(target);
  await changePage(target, letInChange(environment.revision, holders, offer, null));
  return true;
}

/**
 * The request that asks the environment's owner for the same entries, for each person who was named: one item for
 * each, as many as a request takes. A group or a role cannot be named in a request, so those who are asked for are
 * the people among the holders.
 */
export function askToLetIn(offer: Offer, holders: readonly HolderInput[], reason: string, forThread: string): AccessRequestCreateBody | null {
  const permissions = [...VIEW_ROWS, ...offer.askManage].filter((name) => (offer.mayView ? offer.askManage.includes(name) : true));
  const people = holders.filter((holder): holder is Extract<HolderInput, { kind: 'user' }> => holder.kind === 'user');
  if (people.length === 0 || permissions.length === 0) return null;
  return {
    items: people.map((person) => ({ object: { kind: 'environment', id: offer.environment.id }, permissions, holder: person })),
    reason,
    for_thread: forThread,
  };
}
