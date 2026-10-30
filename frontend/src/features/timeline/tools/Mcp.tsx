/** MCP arguments and results.
 *
 * An MCP tool's call: the arguments as a tree, keys and values with long strings folded, and what
 * came back: text as text, anything structured as a tree. An error is shown in the danger style, in
 * the server's own words.
 */
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { InlineAlert } from '@/ui/InlineAlert';
import { JsonTree } from '@/ui/JsonTree';

type McpItem = Extract<Item, { kind: 'tool_call' }>;

export function McpDisplay({ item }: { item: McpItem }) {
  const { t } = useTranslation();
  const tool = item.payload.tool;
  if (tool.kind !== 'mcp') return null;
  return (
    <div className="flex flex-col gap-2">
      <section className="flex flex-col gap-1">
        <h2 className="text-xs font-medium text-text-muted">{t('timeline.mcp.arguments')}</h2>
        <JsonTree value={tool.arguments} label={t('timeline.mcp.arguments')} />
      </section>
      {tool.error !== null && <InlineAlert tone="danger">{tool.error}</InlineAlert>}
      {tool.result !== null && tool.result !== undefined && (
        <section className="flex flex-col gap-1">
          <h2 className="text-xs font-medium text-text-muted">{t('timeline.mcp.result')}</h2>
          {typeof tool.result === 'string' ? (
            <pre className="max-h-(--output-height) overflow-auto rounded-lg border border-border bg-bg-inset p-3 text-sm break-words whitespace-pre-wrap">{tool.result}</pre>
          ) : (
            <JsonTree value={tool.result} label={t('timeline.mcp.result')} />
          )}
        </section>
      )}
    </div>
  );
}
