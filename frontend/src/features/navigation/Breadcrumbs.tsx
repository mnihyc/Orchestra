/** Redacted hierarchy paths, with every org on the way from the root one and the middle folded.
 *
 * Where the screen in view lies. For the screen of a node it is the way from the root org down to
 * it, each place a link with the icon of its kind in its colour, drawn from the org's tree. Where the
 * tree does not hold the whole way (the reader was let into a thing whose place they may not see),
 * the way is the node's own, as the server gives it: a place the reader may not see is named by its
 * kind alone ("Private project"), is no link, and the way goes on past it. A document shows the way
 * to the place it is written beside, then its own title. For any other screen it is the screen's
 * name, with the places above it before it: each a link where the screen says where they lead, or
 * the section it belongs to, as a name, where it does not.
 */
import { useQuery } from '@tanstack/react-query';
import { useRouter } from '@tanstack/react-router';
import { Lock } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import type { NodeKind, ThreadDisplay } from '@/generated/api-types';
import { useNodePath, useTreeStore } from '@/realtime/stores/tree';
import { useCurrentHeading } from '@/shell/screen';
import { screenPath } from '@/shell/screenPath';
import { trailHref } from '@/shell/TrailLink';
import { Breadcrumbs as Trail } from '@/ui/Breadcrumbs';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { NodeIcon } from '@/ui/NodeIcon';

/** A place on the way to the screen in view, as far as its reader may know it. */
export interface WayPart {
  display?: ThreadDisplay | undefined;
  id: string;
  kind: NodeKind;
  /** Its name; `null` for one the reader may not see. */
  name: string | null;
  /** Whether it is a project that lies in a project. */
  folder: boolean;
  /** The colour somebody gave its icon, where a tree held knows it. */
  color: number | null;
  /** A colour of its own for its icon, where it has one and a tree held knows it. */
  own: string | undefined;
  /** For a topic: whether it is done. */
  done: boolean;
}

/** What is needed of a place to lay out the way: a row of the tree and a reference of a node's own path both have it. */
interface Named {
  id: string;
  kind: NodeKind;
  name: string | null;
  redacted?: boolean;
  thread_display?: ThreadDisplay | null;
  thread?: { display?: ThreadDisplay } | null;
}

/**
 * The way to a node, outermost first. The tree's is taken where it reaches a root org: it is live, and
 * a place renamed a moment ago reads right. Otherwise the node's own way is taken, which names every
 * place down from the root org and leaves out the name of each the reader may not see.
 */
export function wayTo(
  fromTree: readonly (Named & { parent_id: string | null })[],
  own: readonly Named[] | undefined,
  rowOf: (id: string) => { name: string; color: number | null; custom_color?: string | null; topic: { status: string } | null; thread?: { display?: ThreadDisplay } | null } | undefined,
): WayPart[] {
  const whole = fromTree.length > 0 && fromTree[0]?.parent_id === null;
  const parts: readonly Named[] = whole || own === undefined ? fromTree : own;
  return parts.map((part, index) => {
    const row = rowOf(part.id);
    const hidden = part.redacted === true || part.name === null;
    return {
      display: hidden ? undefined : (row?.thread?.display ?? part.thread_display ?? part.thread?.display),
      id: part.id,
      kind: part.kind,
      name: hidden ? null : (row?.name ?? part.name),
      folder: part.kind === 'project' && parts[index - 1]?.kind === 'project',
      color: hidden ? null : (row?.color ?? null),
      own: hidden ? undefined : (row?.custom_color ?? undefined),
      done: !hidden && row?.topic?.status === 'done',
    };
  });
}

export function Breadcrumbs({ className }: { className?: string }) {
  const { t } = useTranslation();
  const router = useRouter();
  const heading = useCurrentHeading();
  const nodeId = heading?.nodeId;
  const { path } = useNodePath(nodeId);
  const byOrg = useTreeStore((state) => state.byOrg);
  const broken = nodeId !== undefined && (path.length === 0 || path[0]?.parent_id !== null);
  const own = useQuery({ ...nodeQuery(nodeId ?? ''), enabled: broken && heading?.path === undefined }).data;
  if (heading === null) return <div className={className} />;

  const rowOf = (id: string) => {
    for (const tree of byOrg.values()) {
      const row = tree.nodes.get(id);
      if (row !== undefined && !row.deleted) return row;
    }
    return undefined;
  };
  const way = heading.path === undefined
    ? wayTo(path, broken ? own?.path : undefined, rowOf)
    : wayTo([], heading.path, rowOf);
  const authorized = way.map((part) => ({
    id: part.id,
    label: part.name ?? t(`navigation.private.${part.kind === 'project' && part.folder ? 'folder' : part.kind}` as 'navigation.private.project'),
    href: part.name === null ? undefined : part.kind === 'topic' ? `/topic/${part.id}?tab=overview` : `/${part.kind}/${part.id}`,
  }));
  const above = (heading.trail ?? []).map((part, index) => ({ id: `trail-${index}`, label: part.label, href: trailHref(router, part) }));
  const resolved = screenPath(heading, authorized, above);
  const crumbs = resolved.map((step) => {
    const part = way.find((candidate) => candidate.id === step.id);
    const href = step.href;
    const icon = part?.name === null ? <Lock size={16} aria-hidden />
      : part === undefined ? undefined
      : part.kind === 'thread' ? <ThreadStateIcon display={part.display} />
      : <NodeIcon kind={part.kind === 'project' && part.folder ? 'folder' : part.kind} color={part.color} own={part.own} done={part.done} />;
    return { ...step, icon, important: step.id === heading.nodeId, ...(href === undefined ? {} : { go: () => router.history.push(href) }) };
  });
  return <div className={className}><Trail crumbs={crumbs} max={crumbs.length} /></div>;
}
