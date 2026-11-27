/** A row for each of Orch's tools: what was listed, read and searched, with the place and the depth read; a download; a document made or changed, with Show changes; a message written in a chat or sent to a thread; an announcement or a milestone; a capability asked for.
 *
 * A call of one of Orch's own tools: the line the server gave it, what it was called with and what it
 * returned, as trees. The tools that list, read and search what the team shared, download, write
 * documents and chats, send to threads, announce and ask for a capability are all told this way.
 */
import { useTranslation } from 'react-i18next';
import { useState } from 'react';

import { useBrowserRequests } from '@/features/browser/store';
import { revealView } from '@/features/workbench/openView';
import { CapabilityHelper } from '@/features/timeline/tools/CapabilityHelper';
import { CapabilityFailure } from '@/features/timeline/tools/CapabilityFailure';
import { RecordedSettingSourceLink } from '@/features/settings/RecordedSettingSourceLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import type { Item } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { JsonTree } from '@/ui/JsonTree';

type OrchItem = Extract<Item, { kind: 'tool_call' }>;

/** The tools whose answer names something a view shows: a browser, a display. */
const SHOWS_A_VIEW: readonly string[] = ['use_browser', 'use_display'];

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

export function OrchDisplay({ item }: { item: OrchItem }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const [factsOpen, setFactsOpen] = useState(false);
  const tool = item.payload.tool;
  if (tool.kind !== 'orch') return null;
  const args = record(tool.arguments) ? tool.arguments : {};
  const threadMessage = tool.tool === 'send_message' && record(args.target) && args.target.kind === 'thread';
  const applied = tool.tool === 'announce' && record(tool.result) && record(tool.result.applied) ? tool.result.applied : null;
  const wake = threadMessage ? args.wake_idle === true : applied !== null ? applied.wake_idle === true : null;
  const rate = tool.rate_limit;
  const result = record(tool.result) && record(tool.result.structuredContent) ? tool.result.structuredContent : tool.result;
  const rateRefused = rate !== undefined && rate !== null || record(result) && record(result.error) && result.error.code === 'rate_limited';
  const wait = rate?.retry_at === null || rate?.retry_at === undefined ? undefined : Date.parse(rate.retry_at) - Date.parse(rate.observed_at);
  return (
    <div className="flex flex-col gap-2">
      <p className="font-mono text-sm text-text-muted">{tool.tool}{wake !== null && <> · <span className="font-sans">{t(wake ? 'composer.wake.requested' : 'composer.wake.notRequested')}</span></>}</p>
      {rateRefused && <FactDisclosure open={factsOpen} onOpenChange={setFactsOpen} fragment={t('composer.toolRate.refused')} groups={[{ key: 'tool-rate', kind: 'recorded_outcome', facts: [
        { key: 'wait', label: t('composer.toolRate.wait'), value: wait === undefined || !Number.isFinite(wait) ? undefined : formatSpan(Math.max(0, wait), format, 'short') },
        { key: 'allowance', label: t('composer.toolRate.allowance'), value: rate === undefined || rate === null ? undefined : t('composer.toolRate.calls', { count: rate.allowed_calls, number: formatNumber(rate.allowed_calls, format.locale), period: formatSpan(rate.period_secs * 1_000, format, 'short') }) },
        { key: 'source', label: t('composer.toolRate.source'), value: rate === undefined || rate === null || !factsOpen ? undefined : <RecordedSettingSourceLink source={rate.source} setting="orch_tools.calls_per_minute" target={{ kind: 'node', id: item.thread_id }} /> },
        { key: 'observed', label: t('composer.toolRate.observed'), value: rate === undefined || rate === null || !factsOpen ? undefined : <RelativeTime at={rate.observed_at} /> },
      ] }]} />}
      {tool.tool === 'image_generation' && <CapabilityHelper result={tool.result} />}
      <CapabilityFailure result={tool.result} />
      <JsonTree value={tool.arguments} label={t('timeline.mcp.arguments')} />
      {tool.result !== null && tool.result !== undefined && <JsonTree value={tool.result} label={t('timeline.mcp.result')} />}
      {SHOWS_A_VIEW.includes(tool.tool) && <div><OpenFromTool tool={tool.tool} result={tool.result} /></div>}
    </div>
  );
}

/** "Open" on the row of the agent's tool: the browser or display the answer names, in front of its view. */
function OpenFromTool({ tool, result }: { tool: string; result: unknown }) {
  const { t } = useTranslation();
  if (typeof result !== 'object' || result === null) return null;
  const record = result as Record<string, unknown>;
  const browser = tool === 'use_browser' && typeof record.browser === 'string' ? record.browser : null;
  const display = tool === 'use_display' && typeof record.display === 'string' ? record.display : null;
  if (browser === null && display === null) return null;
  return (
    <Button
      action="open"
      size="sm"
      onClick={() => {
        if (browser !== null) {
          useBrowserRequests.getState().showBrowser(browser);
          revealView('browser');
        } else if (display !== null) {
          useBrowserRequests.getState().showDisplay(display);
          revealView('vnc');
        }
      }}
    >
      {browser !== null ? t('browser.tool.open') : t('vnc.tool.open')}
    </Button>
  );
}
