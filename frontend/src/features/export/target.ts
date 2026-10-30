/** What can be taken away: a thread, a chat or a document as one file, and an org, a project, a folder or a topic as one archive.
 *
 * The thing that Export… was pressed on, as the dialog needs to know it: its kind, its identifier and its name.
 * A thread, a chat and a document give one file at once; a place gives an archive of what its person may read in
 * it, which is planned, asked for and written by a job.
 */
import type { NodeKind, UserRef } from '@/generated/api-types';

/** What Export… was pressed on. */
export type ExportKind = NodeKind | 'chat' | 'document';

export interface ExportTarget {
  kind: ExportKind;
  id: string;
  name: string;
  /** Whom to ask for the right to take it away, where the person has none: its first owner. */
  owner?: Pick<UserRef, 'display_name'> | null;
  /** The object whose page holds the right, where it is not the thing itself: the node a chat of a node belongs to. */
  page?: { kind: string; id: string };
}

/** The kinds of place: what Export… makes an archive of. A project that lies in a project is a folder and is a project all the same. */
export type PlaceKind = Extract<NodeKind, 'org' | 'project' | 'topic'>;

/** Whether the target is a place, which is exported as an archive. */
export function isPlace(target: ExportTarget): target is ExportTarget & { kind: PlaceKind } {
  return target.kind === 'org' || target.kind === 'project' || target.kind === 'topic';
}
