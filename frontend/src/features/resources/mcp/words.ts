/** MCP status reasons use catalogue words; masked native diagnostics remain details on intent. */
import type { TFunction } from 'i18next';
import type { McpState } from '@/generated/api-types';
import type { TranslationKey } from '@/i18n';

export function mcpProgram(source: string): string {
  return source === 'codex' ? 'Codex' : source === 'claude' || source === 'claude_code' || source === 'claude-code' ? 'Claude Code' : source;
}

const reasons = new Map<string, TranslationKey>([
  ['this server is switched off', 'mcpWords.reasons.disabled'],
  ['this server belongs to another environment', 'mcpWords.reasons.otherEnvironment'],
  ['no machine', 'mcpWords.reasons.noMachine'],
  ['its imported definition belongs to another machine', 'mcpWords.reasons.otherMachine'],
  ['the machine permits no mcp server added from here', 'mcpWords.reasons.addOff'],
  ['the machine permits no imported mcp server', 'mcpWords.reasons.importOff'],
  ['another enabled server has the same name', 'mcpWords.reasons.nameConflict'],
  ['whoever enabled it may no longer use it', 'mcpWords.reasons.accessLost'],
  ['a required capability is not granted to this environment', 'mcpWords.reasons.capability'],
  ['policy.mcp_add_disabled', 'mcpWords.reasons.addOff'],
  ['policy.mcp_import_disabled', 'mcpWords.reasons.importOff'],
  ['agent.offline', 'mcpWords.reasons.offline'],
  ['source.offline', 'mcpWords.reasons.offline'],
  ['mcp.disabled', 'mcpWords.reasons.disabled'],
  ['mcp.other_environment', 'mcpWords.reasons.otherEnvironment'],
  ['mcp.no_machine', 'mcpWords.reasons.noMachine'],
  ['mcp.other_machine', 'mcpWords.reasons.otherMachine'],
  ['mcp.add_disabled', 'mcpWords.reasons.addOff'],
  ['mcp.import_disabled', 'mcpWords.reasons.importOff'],
  ['mcp.name_conflict', 'mcpWords.reasons.nameConflict'],
  ['mcp.access_lost', 'mcpWords.reasons.accessLost'],
  ['mcp.capability_required', 'mcpWords.reasons.capability'],
  ['mcp.command_missing', 'mcpWords.reasons.commandMissing'],
  ['mcp.machine_offline', 'mcpWords.reasons.offline'],
  ['mcp.failed', 'mcpWords.reasons.failed'],
  ['mcp.unavailable', 'mcpWords.reasons.unavailable'],
]);

export function mcpReasonCode(value: object): string | undefined {
  return 'reason_code' in value && typeof value.reason_code === 'string' ? value.reason_code : undefined;
}

export function mcpReason(reason: string | null, state: McpState, t: TFunction, code?: string): { text: string; details: string | null } | null {
  if (code !== undefined) return { text: t(reasons.get(code.trim().toLowerCase()) ?? (state === 'failed' ? 'mcpWords.reasons.failed' : 'mcpWords.reasons.unavailable')),
    details: reason === null || reason.trim() === '' ? null : reason };
  if (reason === null || reason.trim() === '') return null;
  const key = reasons.get(reason.trim().toLowerCase());
  return key === undefined
    ? { text: t(state === 'failed' ? 'mcpWords.reasons.failed' : 'mcpWords.reasons.unavailable'), details: reason }
    : { text: t(key), details: null };
}
