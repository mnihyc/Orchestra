/** A changed native record repairs its owner's searched pages, whose membership belongs to the server. */
import type { Change, MissingProjection, PathParam } from '@/api/acts';
import type { Act } from '@/api/client';
import { record } from '@/api/cache';

export function searchedPageChanges(act: Act, p: PathParam): { early: Change[]; late: Change[] } {
  const parts = act.path.split('/').filter(Boolean).map(decodeURIComponent);
  const parameter = p('id');
  const id = parameter === '' ? parts[1] ?? '' : parameter;
  const early: Change[] = [], late: Change[] = [];
  const page = (key: readonly [string, ...unknown[]]) => late.push({ missing: [{ key }], allObserved: true, cancelPending: true });
  const containing = (kind: string, row: string, account = false) => early.push({
    missing: [{ key: [kind], at: { index: 2, value: 'pages' } }], allObserved: true, cancelPending: true,
    ...(account ? { containingAccount: row } : { containingRow: row }),
  } satisfies MissingProjection);
  if (parts[0] === 'admin' && parts[1] === 'identity-providers') page(['admin', 'identity-providers', 'pages']);
  if (parts[0] === 'admin' && parts[1] === 'agents') page(['admin-system-agents']);
  if (parts[0] === 'nodes' && parts[2] === 'destinations') page(['destinations', id, 'pages']);
  if (parts[0] === 'destinations' && parts.length >= 2) {
    if (record(act.answer) && record(act.answer.node) && typeof act.answer.node.id === 'string') page(['destinations', act.answer.node.id, 'pages']);
    else containing('destinations', id);
  }
  if (parts[0] === 'me' && parts[1] === 'push-subscriptions') page(['away', 'devices', 'pages']);
  if (parts[0] === 'me' && parts[1] === 'notification-routes' && act.method === 'DELETE') page(['away', 'routes', 'retained', 'pages']);
  if (parts[0] === 'topics' && parts[2] === 'input-schedules') page(['input-schedule-pages', id]);
  if (parts[0] === 'input-schedules' && parts.length === 2) {
    if (record(act.answer) && record(act.answer.topic) && typeof act.answer.topic.id === 'string') page(['input-schedule-pages', act.answer.topic.id]);
    else early.push({ missing: [{ key: ['input-schedule-pages'], at: { index: 3, value: 'pages' } }], containingRow: id, allObserved: true, cancelPending: true });
  }
  if (parts[0] === 'invitations') {
    if (act.method === 'POST' && record(act.input) && typeof act.input.org_id === 'string') page(['invitations', act.input.org_id, 'pages']);
    if (act.method === 'DELETE' && parts.length === 2) containing('invitations', id);
  }
  if (parts[0] === 'admin' && parts[1] === 'notification-channels') page(['channels', 'system', 'pages']);
  if (parts[0] === 'orgs' && parts[2] === 'notification-channels') page(['channels', id, 'pages']);
  if (parts[0] === 'me' && parts[1] === 'notification-channels') page(['notification-channels', 'pages']);
  if ((parts[0] === 'admin' || parts[0] === 'orgs' || parts[0] === 'me') && parts.includes('notification-channels') && act.method === 'POST') page(['notification-channels', 'pages']);
  if (parts[0] === 'notification-channels' && parts.length >= 2) {
    if (record(act.answer) && act.answer.owner === 'system') page(['channels', 'system', 'pages']);
    else if (record(act.answer) && record(act.answer.owner) && record(act.answer.owner.org) && typeof act.answer.owner.org.id === 'string') page(['channels', act.answer.owner.org.id, 'pages']);
    else containing('channels', id);
    early.push({ missing: [{ key: ['notification-channels', 'pages'] }], containingRow: id, allObserved: true, cancelPending: true });
  }
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
