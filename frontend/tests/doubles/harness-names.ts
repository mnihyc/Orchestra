/** What the API says of a program's name beside its kind, as the registry of the shipped programs gives it: the name in English and in the other languages. A kind nobody described is called by its kind. */
import type { AgentDescriptor, HarnessKindName } from '../../src/generated/api-types.ts';

const SHIPPED: Record<string, { name: string; names: Record<string, string> }> = {
  codex: { name: 'Codex', names: { 'zh-CN': 'Codex' } },
  'claude-code': { name: 'Claude Code', names: { 'zh-CN': 'Claude Code' } },
  fake: { name: 'Example agent', names: { 'zh-CN': '示例代理' } },
};

/** The named kind a payload carries beside `harness_kind`. */
export function harnessName(kind: string): HarnessKindName {
  const known = SHIPPED[kind];
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
