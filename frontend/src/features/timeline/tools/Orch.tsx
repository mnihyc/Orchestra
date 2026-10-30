/** A row for each of Orch's tools: what was listed, read and searched, with the place and the depth read; a download; a document made or changed, with Show changes; a message written in a chat or sent to a thread; an announcement or a milestone; a capability asked for.
 *
 * A call of one of Orch's own tools: the line the server gave it, what it was called with and what it
 * returned, as trees. The tools that list, read and search what the team shared, download, write
 * documents and chats, send to threads, announce and ask for a capability are all told this way.
 */
import { useTranslation } from 'react-i18next';

import { OpenFromTool } from '@/features/browser/View';
import type { Item } from '@/generated/api-types';
import { JsonTree } from '@/ui/JsonTree';

type OrchItem = Extract<Item, { kind: 'tool_call' }>;

/** The tools whose answer names something a view shows: a browser, a display. */
const SHOWS_A_VIEW: readonly string[] = ['use_browser', 'use_display'];

export function OrchDisplay({ item }: { item: OrchItem }) {
  const { t } = useTranslation();
  const tool = item.payload.tool;
  if (tool.kind !== 'orch') return null;
  return (
    <div className="flex flex-col gap-2">
      <p className="font-mono text-sm text-text-muted">{tool.tool}</p>
      <JsonTree value={tool.arguments} label={t('timeline.mcp.arguments')} />
      {tool.result !== null && tool.result !== undefined && <JsonTree value={tool.result} label={t('timeline.mcp.result')} />}
      {SHOWS_A_VIEW.includes(tool.tool) && <div><OpenFromTool tool={tool.tool} result={tool.result} /></div>}
    </div>
  );
}
