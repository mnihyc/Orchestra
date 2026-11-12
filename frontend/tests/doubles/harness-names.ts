/** The names declared by shipped and registered fixture programs. A kind nobody described is called by its kind. */
import type { AgentDescriptor, HarnessKindName } from '../../src/generated/api-types.ts';

const DECLARED: Record<string, { name: string; names: Record<string, string> }> = {
  codex: { name: 'Codex', names: { 'zh-CN': 'Codex' } },
  'claude-code': { name: 'Claude Code', names: { 'zh-CN': 'Claude Code' } },
  fake: { name: 'Example agent', names: { 'zh-CN': '示例代理' } },
  'acme-agent': { name: 'Acme agent', names: {} },
};

/** The named kind a payload carries beside `harness_kind`. */
export function harnessName(kind: string): HarnessKindName {
  const known = DECLARED[kind];
  return { kind, name: known?.name ?? kind, names: known?.names ?? {} };
}

/** The same for a payload whose kind may be absent. */
export function maybeHarnessName(kind: string | null): HarnessKindName | null {
  return kind === null ? null : harnessName(kind);
}

/** A descriptor as a test writes it, with only the members it needs: the API's type has them all. */
export function partialDescriptor(value: object): AgentDescriptor {
  return value as AgentDescriptor;
}
