/** The move chooser presents folders and topics of this tree without offering a conversation or a hidden branch. */
import { describe, expect, it } from 'vitest';
import { topicTree, type MovePlace } from '@/features/thread-move/tree';

const place = (id: string, kind: MovePlace['kind'], parent_id: string | null, more: Partial<MovePlace> = {}): MovePlace => ({ id, kind, parent_id, name: id, color: null, archived: false, deleted: false, ...more });
const nodes = new Map([
  place('root', 'org', null), place('web', 'project', 'root'), place('payments', 'project', 'web'),
  place('refunds', 'topic', 'payments'), place('conversation', 'thread', 'refunds'),
  place('old', 'project', 'root', { archived: true }), place('old-topic', 'topic', 'old'),
  place('deleted', 'topic', 'web', { deleted: true }), place('another-root', 'org', null), place('elsewhere', 'topic', 'another-root'),
].map((entry) => [entry.id, entry]));

describe('choosing a topic to move to', () => {
  it('shows the folder ancestry and topic leaves of this root alone', () => {
    const rows = topicTree(nodes, 'root', new Set(['root', 'web', 'payments']));
    expect(rows.map((row) => [row.id, row.depth])).toEqual([['root', 0], ['web', 1], ['payments', 2], ['refunds', 3]]);
    expect(rows.at(-1)).toMatchObject({ expandable: false, expanded: false });
  });
  it('keeps a closed folder visible without listing its descendants until it opens', () => {
    expect(topicTree(nodes, 'root', new Set(['root', 'web'])).map((row) => row.id)).toEqual(['root', 'web', 'payments']);
    expect(topicTree(nodes, 'root', new Set()).map((row) => row.id)).toEqual(['root']);
  });
  it('does not invent a root or expose disconnected rows while the tree is unavailable', () => {
    expect(topicTree(nodes, 'unreadable', new Set(['web']))).toEqual([]);
  });
});
