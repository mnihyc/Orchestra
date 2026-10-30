/** Approval cards ignore a thin or foreign cached row while retaining their native request snapshot. */
import type { Approval } from '@/generated/api-types';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const words = (value: unknown): value is string => typeof value === 'string';
const nullableWords = (value: unknown): boolean => value === null || words(value);
const strings = (value: unknown): boolean => Array.isArray(value) && value.every((entry: unknown) => words(entry));

function option(value: unknown): boolean {
  if (!record(value) || !words(value.id) || !words(value.label) || !words(value.style) || !['allow', 'deny', 'neutral'].includes(value.style)
    || value.scope !== null && (!words(value.scope) || !['once', 'session', 'always', 'thread'].includes(value.scope))) return false;
  if (value.rule === null) return true;
  const rule = value.rule;
  if (!record(rule)) return false;
  switch (rule.type) {
    case 'command_prefix': return words(rule.cwd) && strings(rule.argv);
    case 'command_exact': return words(rule.cwd) && words(rule.command);
    case 'host': return words(rule.host) && words(rule.protocol) && typeof rule.port === 'number';
    default: return false;
  }
}

function request(value: Record<string, unknown>): boolean {
  const detail = value.detail;
  if (!record(detail) || !Array.isArray(value.options) || !value.options.every((entry: unknown) => option(entry))) return false;
  switch (value.request_kind) {
    case 'command': return words(detail.command) && words(detail.cwd) && nullableWords(detail.reason);
    case 'file_change': return nullableWords(detail.reason) && Array.isArray(detail.changes) && detail.changes.every((change: unknown) => record(change) && words(change.path)
      && words(change.change) && ['add', 'modify', 'delete', 'rename'].includes(change.change) && nullableWords(change.old_path) && nullableWords(change.diff)
      && typeof change.added === 'number' && typeof change.removed === 'number' && typeof change.binary === 'boolean' && typeof change.truncated === 'boolean');
    case 'permissions': return 'permissions' in detail && nullableWords(detail.reason);
    case 'other': return words(detail.summary);
    case 'plan': return typeof detail.plan === 'string';
    case 'mcp_elicitation': return record(detail.schema) && words(detail.message);
    case 'user_input': return Array.isArray(detail.questions) && detail.questions.every((question: unknown) => record(question) && words(question.id) && words(question.header) && words(question.question)
      && typeof question.multiple === 'boolean' && typeof question.allow_other === 'boolean'
      && (question.options === null || Array.isArray(question.options) && question.options.every((choice: unknown) => record(choice) && words(choice.label) && words(choice.description))));
    default: return false;
  }
}

/** Only the matching request's recognizable native shape is a renderable approval. */
function valid(value: unknown, id: string): value is Approval {
  if (!record(value) || value.id !== id || !words(value.title) || !words(value.created_at) || !Number.isFinite(Date.parse(value.created_at))
    || typeof value.can_decide !== 'boolean' || !record(value.request)) return false;
  if (!words(value.state) || !['pending', 'approved', 'answered', 'denied', 'expired', 'cancelled'].includes(value.state)) return false;
  if (value.decided_by !== null && (!record(value.decided_by) || !words(value.decided_by.display_name))) return false;
  if (value.decided_at !== null && (!words(value.decided_at) || !Number.isFinite(Date.parse(value.decided_at)))) return false;
  switch (value.kind) {
    case 'harness': return request(value.request);
    case 'capability': return words(value.request.capability) && words(value.request.label) && words(value.request.reason) && strings(value.request.consequences);
    case 'access': return true;
    case 'port': return record(value.request.plan) && record(value.request.request) && typeof value.request.request.port === 'number'
      && record(value.request.opening) && record(value.request.environment);
    default: return false;
  }
}

export function approvalSnapshot(value: unknown, id: string): Approval | undefined {
  return valid(value, id) ? value : undefined;
}
