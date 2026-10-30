/** Child agent activity.
 *
 * A sub-agent the agent started: its name and role, what it was asked to do and where it stands. Its
 * own stream opens under this step in an indented lane, using the same displays at compact density.
 */
import type { Item } from '@/generated/api-types';
import { SubagentLane } from '@/features/timeline/SubagentLane';

type SubagentItem = Extract<Item, { kind: 'tool_call' }>;

export function SubagentDisplay({ item }: { item: SubagentItem }) {
  return <SubagentLane item={item} />;
}
