/** Unknown or additional declared tool payloads.
 *
 * A tool the page has no display for, or one an adapter added: its own name, what it was called with
 * and what it returned, as trees, so that nothing the agent did is out of reach.
 */
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { JsonTree } from '@/ui/JsonTree';
import { ApprovalReviewDisplay } from './ApprovalReview';

type OtherItem = Extract<Item, { kind: 'tool_call' }>;

export function OtherDisplay({ item }: { item: OtherItem }) {
  const { t } = useTranslation();
  const tool = item.payload.tool;
  if (tool.kind !== 'other') return null;
  if (tool.name === 'harness/approvalReview') return <ApprovalReviewDisplay value={tool.result} />;
  return (
    <div className="flex flex-col gap-2">
      <p className="font-mono text-sm text-text-muted">{tool.name}</p>
      <JsonTree value={tool.arguments} label={t('timeline.mcp.arguments')} />
      {tool.result !== null && tool.result !== undefined && <JsonTree value={tool.result} label={t('timeline.mcp.result')} />}
    </div>
  );
}
