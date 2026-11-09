/** Complete native place rows share one set of fixture defaults. Reader projections override their own facts. */
import type { TreeNode, TreeNodeListedContent } from '../../src/generated/api-types.ts';
import { executionDisplay } from './thread-display.ts';

type Seed = Pick<TreeNode, 'id' | 'kind' | 'parent_id'> & Partial<TreeNode>;
type ThreadRow = NonNullable<TreeNode['thread']>;
type ThreadSeed = Omit<ThreadRow, 'display' | 'attention'> & Partial<Pick<ThreadRow, 'display' | 'attention'>>;
export type TreeChange = Omit<Partial<TreeNode>, 'thread'> & { id: string; thread?: ThreadSeed | null };

/** Controls may give execution facts; complete them before a native reader observes the row. */
export function treeThread(seed: ThreadSeed): ThreadRow {
  return {
    ...seed,
    display: seed.display ?? executionDisplay(seed.state, { seq: seed.activity_seq, blocked_reason: seed.blocked_reason }),
    attention: seed.attention ?? { uncommitted: null, unpushed: null, failed: false, as_of: null, stale: false, reason: null },
  };
}

export function treeNode(seed: Seed): TreeNode & { participating: boolean } {
  return {
    address: null,
    name: seed.id,
    color: null,
    docs: 0,
    archived: false,
    deleted: false,
    restricted: false,
    chat: { id: `chat-${seed.id}`, seq: 0 },
    thread: null,
    topic: null,
    tree_seq: 0,
    ...seed,
    docs_seq: seed.docs_seq ?? 0,
    participating: seed.participating ?? true,
  };
}

/** A disclosed list member carries its own required wire discriminator. */
export function listedTreeNode(seed: Seed): TreeNodeListedContent & { participating: boolean } {
  return { ...treeNode(seed), content_hidden: false };
}
