/** History vocabulary comes from structured facts; a server sentence is never parsed into an action. */
import type { TFunction } from 'i18next';
import type { HistoryLine } from '@/generated/api-types';

const KEYS = {
  status: 'places.activity.statusChanged', roles: 'places.activity.rolesChanged', said: 'places.activity.said', work: 'places.activity.ranWork', signIn: 'places.activity.signInChanged', turnCompleted: 'places.activity.turnCompleted', turnFailed: 'places.activity.turnFailed', milestone: 'places.activity.milestone',
  defaults: 'access.history.words.defaults', agentAccess: 'access.history.words.agentAccess', agentDefaults: 'access.history.words.agentDefaults',
  changed: 'access.history.words.changed',
  created: 'access.history.words.created', renamed: 'access.history.words.renamed', described: 'access.history.words.described', model: 'access.history.words.model', setting: 'access.history.words.setting', access: 'access.history.words.access', owner: 'access.history.words.owner', moved: 'access.history.words.moved', forked: 'access.history.words.forked', switched: 'access.history.words.switched', started: 'access.history.words.started', stopped: 'access.history.words.stopped', archived: 'access.history.words.archived', unarchived: 'access.history.words.unarchived', deleted: 'access.history.words.deleted', restored: 'access.history.words.restored', shared: 'access.history.words.shared', unshared: 'access.history.words.unshared', decided: 'access.history.words.decided', held: 'access.history.words.held', resumed: 'access.history.words.resumed', files: 'access.history.words.files', capabilityGranted: 'access.history.words.capabilityGranted', capabilityRevoked: 'access.history.words.capabilityRevoked', tools: 'access.history.words.tools', recreated: 'access.history.words.recreated', ruleAdded: 'access.history.words.ruleAdded', ruleRevoked: 'access.history.words.ruleRevoked',
} as const;

/** Opaque identifiers in old audit summaries are not names. Unknown vocabulary keeps only safe fallback text. */
export function historySentence(line: Pick<HistoryLine, 'actor' | 'text' | 'wording'>, t: TFunction, objectName?: string): string {
  const actor = line.actor === null ? t('app.name') : 'agent' in line.actor ? t('access.history.agent', { name: line.actor.agent.name ?? t('access.history.removedThread') }) : line.actor.display_name;
  const words = line.wording;
  const name = objectName ?? words?.target?.name ?? t(words?.target?.kind === 'thread' ? 'access.history.removedThread' : 'access.history.thisPlace');
  const key = words?.version === 1 ? Object.entries(KEYS).find(([verb]) => verb === words.verb)?.[1] : undefined;
  if (key !== undefined) return t(key, { actor, name });
  return /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i.test(line.text) ? t('access.history.recorded', { actor }) : line.text;
}
