/** A changed native record repairs its owner's searched pages, whose membership belongs to the server. */
import type { Change, MissingProjection, PathParam } from '@/api/acts';
import type { Act } from '@/api/client';
import { record } from '@/api/cache';

export function searchedPageChanges(act: Act, p: PathParam): { early: Change[]; late: Change[] } {
  const parts = act.path.split('/').filter(Boolean).map(decodeURIComponent);
  const id = p('id') || parts[1] || '';
  const early: Change[] = [], late: Change[] = [];
  const page = (key: readonly [string, ...unknown[]]) => late.push({ missing: [{ key }], allObserved: true, cancelPending: true });
  const containing = (kind: string, row: string, account = false) => early.push({
    missing: [{ key: [kind], at: { index: 2, value: 'pages' } }], allObserved: true, cancelPending: true,
    ...(account ? { containingAccount: row } : { containingRow: row }),
  } satisfies MissingProjection);
  if (parts[0] === 'me' && parts[1] === 'tokens') page(['tokens', 'pages']);
  if (parts[0] === 'me' && parts[1] === 'sessions' && parts.length === 3) page(['sessions', 'pages']);
  if (parts[0] === 'me' && parts[1] === 'password') page(['sessions', 'pages']);
  if (parts[0] === 'orgs' && parts[2] === 'groups' && parts.length === 3) page(['groups', id, 'pages']);
  if (parts[0] === 'groups' && parts.length === 2) {
    if (act.method === 'DELETE') containing('groups', id);
    else if (record(act.answer) && typeof act.answer.org_id === 'string') page(['groups', act.answer.org_id, 'pages']);
  }
  if (parts[0] === 'groups' && ['holders', 'members'].includes(parts[2] ?? '')) {
    page(['group-holders', id, 'pages']); containing('groups', id);
  }
  if (parts[0] === 'orgs' && parts[2] === 'service-accounts') page(['service-accounts', id, 'pages']);
  if (parts[0] === 'service-accounts' && parts.length === 2 && act.method === 'DELETE') containing('service-accounts', id, true);
  if (parts[0] === 'service-accounts' && parts[2] === 'tokens') {
    page(['service-tokens', id, 'pages']); containing('service-accounts', id, true);
  }
  if (parts[0] === 'nodes' && parts[2] === 'tags' && act.method === 'POST') page(['tags', id, 'pages']);
  if (parts[0] === 'tags' && parts.length === 2) {
    if (act.method === 'DELETE') containing('tags', id);
    else if (record(act.answer) && record(act.answer.node) && typeof act.answer.node.id === 'string') page(['tags', act.answer.node.id, 'pages']);
  }
  return { early, late };
}
