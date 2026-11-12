/** MCP availability and kept plan entries use catalogue words in both languages. */
import { expect, it } from 'vitest';
import { mcpLines, staysOf, type PlanContext } from '@/features/change/plan';
import { mcpProgram, mcpReason } from '@/features/resources/mcp/words';
import type { ForkPlan, PlanMcpServer } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { ADA } from './general-access-fixtures';
import { resourcePlan } from '../component/plan-resource-fixtures';

for (const locale of ['en', 'zh-CN'] as const) {
  it(`translates current legacy reasons and stable reason codes (${locale})`, () => {
    const t = i18n.getFixedT(locale);
    expect(mcpReason('its imported definition belongs to another machine', 'unavailable', t))
      .toEqual({ text: t('mcpWords.reasons.otherMachine'), details: null });
    expect(mcpReason('mcp.command_missing', 'unavailable', t))
      .toEqual({ text: t('mcpWords.reasons.commandMissing'), details: null });
    expect(mcpReason('the machine permits no imported MCP server', 'unavailable', t))
      .toEqual({ text: t('mcpWords.reasons.importOff'), details: null });
    expect(mcpReason('a required capability is not granted to this environment', 'unavailable', t)?.text)
      .toBe(t('mcpWords.reasons.capability'));
  });

  it(`keeps unknown diagnostics out of the primary reason (${locale})`, () => {
    const t = i18n.getFixedT(locale);
    const diagnostic = 'native transport returned an unrecognized failure';
    expect(mcpReason(diagnostic, 'failed', t)).toEqual({ text: t('mcpWords.reasons.failed'), details: diagnostic });
    expect(mcpReason('constructor', 'unavailable', t)?.text).toBe(t('mcpWords.reasons.unavailable'));
    expect(mcpReason(' ', 'running', t)).toBeNull();
  });

  it(`prefers a structured class and keeps the actual diagnostic in Details (${locale})`, () => {
    const t = i18n.getFixedT(locale);
    const diagnostic = 'native executable was not found';
    expect(mcpReason(diagnostic, 'unavailable', t, 'mcp.unavailable')).toEqual({ text: t('mcpWords.reasons.unavailable'), details: diagnostic });
    expect(mcpReason(diagnostic, 'failed', t, 'mcp.failed')).toEqual({ text: t('mcpWords.reasons.failed'), details: diagnostic });
    expect(mcpReason('No machine', 'failed', t, 'mcp.future_class')?.text).toBe(t('mcpWords.reasons.failed'));
  });

  it(`keeps an explicitly retained server in the plan's stays list (${locale})`, () => {
    const t = i18n.getFixedT(locale);
    const kept: PlanMcpServer = { id: 'server', name: 'docs', fate: 'kept', reason: null, imported: true, own: false };
    // Only the facts consumed by the stays adapter are needed; this starts no plan or job.
    const plan: ForkPlan = { ...resourcePlan('mcp-words'), result: 'new', mcp: [kept, { ...kept, id: 'lost', fate: 'lost' }] };
    const context: PlanContext = { kind: 'fork', thread: { name: 'Thread', owner: ADA, model: 'route', modelDisplay: 'Model', effort: null }, me: ADA.id, locale, models: [] };
    const lines = staysOf(plan, context, t);
    expect(lines.find((line) => line.id === 'mcp-server')).toMatchObject({ tone: 'plain', text: t('mcpPlan.kept', { name: kept.name }) });
    expect(lines.some((line) => line.id === 'mcp-lost')).toBe(false);
    expect(mcpLines([{ ...kept, fate: 'unavailable', reason: 'No machine' }], t)[0]?.text)
      .toBe(t('mcpPlan.unavailableBecause', { name: kept.name, reason: t('mcpWords.reasons.noMachine') }));
  });
}

it('uses the familiar names of both programs and preserves an unknown supplied name', () => {
  expect(mcpProgram('codex')).toBe('Codex');
  expect(mcpProgram('claude_code')).toBe('Claude Code');
  expect(mcpProgram('local-program')).toBe('local-program');
});
