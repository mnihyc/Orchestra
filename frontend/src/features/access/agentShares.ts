/** What sharing with agents is made of: the three areas a thing has, the everyday choice and what it means for each, how far a share goes, and who may change one.
 *
 * Agents are given the same three words as people, Can view, Can edit and Can manage, and by area the
 * same three columns the page has: their conversations, the people's chats and the documents. A choice
 * means a level in each area the thing has, and a level means rows on the page; this file works out the first
 * and the last from the other two and from what the reader may give (`PermissionPage.gives.agents`), and
 * holds nothing that a request does.
 */
import type { AccessKind } from '@/api/access';
import type { AgentLevel, AgentShare, ShareDocumentsLevel, ShareLevel, ShareParts } from '@/generated/api-types';

import type { Reach } from './Covers';

/** The three areas of a share, in the order a person reads them. */
export const AREAS = ['timeline', 'chat', 'documents'] as const;
export type Area = (typeof AREAS)[number];

/** The everyday choices for agents, from the least to the most. */
export const LEVELS: readonly AgentLevel[] = ['view', 'edit', 'manage'];

/** Which areas a thing has, for sharing with agents: a thread has all three, a place its chat and documents and, with what is in it, the conversations inside; a chat only itself and a document only itself. */
export function areasOf(kind: AccessKind, reach: Reach): readonly Area[] {
  if (kind === 'chat') return ['chat'];
  if (kind === 'document' || kind === 'document_container') return ['documents'];
  if (kind === 'thread') return AREAS;
  return reach === 'alone' ? ['chat', 'documents'] : AREAS;
}

/** What an everyday choice comes to in each area the thing has. Manage adds the document itself and nothing for conversations or chats. */
export function partsOfLevel(level: AgentLevel, areas: readonly Area[]): ShareParts {
  const parts: ShareParts = {};
  for (const area of areas) {
    if (area === 'documents') parts.documents = level === 'view' ? 'read' : level === 'edit' ? 'edit' : 'manage';
    else parts[area] = level === 'view' ? 'read' : 'write';
  }
  return parts;
}

/** The level an area is shared at, as a number: not shared 0, read 1, write or edit 2, manage 3. */
export function stepOf(area: Area, parts: ShareParts): number {
  const level = parts[area];
  if (level === undefined || level === null) return 0;
  if (level === 'read') return 1;
  return level === 'manage' ? 3 : 2;
}

/** The everyday choice that parts amount to; `null` where they do not make one (an area left out, or the areas at different levels). */
export function levelOfParts(parts: ShareParts, areas: readonly Area[]): AgentLevel | null {
  const steps = areas.map((area) => stepOf(area, parts));
  const documents = areas.includes('documents') ? stepOf('documents', parts) : 0;
  if (steps.some((step) => step === 0)) return null;
  if (documents === 3 && steps.every((step) => step === 3 || step === 2)) return 'manage';
  if (steps.every((step) => step === 1)) return 'view';
  if (steps.every((step) => step === 2 || (step === 3 && documents === 3))) return 'edit';
  return null;
}

/** What each level needs the reader to be able to give: read of View, write and edit of Edit, manage of Manage. */
export function levelNeeded(step: number): AgentLevel | null {
  return step === 0 ? null : step === 1 ? 'view' : step === 2 ? 'edit' : 'manage';
}

/** Whether the reader may give parts: every area shared at a level they may give. */
export function mayGiveParts(parts: ShareParts, gives: readonly AgentLevel[]): boolean {
  return AREAS.every((area) => {
    const needed = levelNeeded(stepOf(area, parts));
    return needed === null || gives.includes(needed);
  });
}

/** Whether two sets of parts say the same. */
export function sameParts(one: ShareParts, other: ShareParts): boolean {
  return AREAS.every((area) => stepOf(area, one) === stepOf(area, other));
}

/** The parts with one area set to a step: 0 not shared, 1 read, 2 write (edit for documents), 3 manage. */
export function withStep(parts: ShareParts, area: Area, step: number): ShareParts {
  const next: ShareParts = {};
  const documents: ShareDocumentsLevel | null = area === 'documents' ? (step === 1 ? 'read' : step === 2 ? 'edit' : step === 3 ? 'manage' : null) : (parts.documents ?? null);
  const conversations: ShareLevel | null = area === 'timeline' ? (step === 1 ? 'read' : step >= 2 ? 'write' : null) : (parts.timeline ?? null);
  const chats: ShareLevel | null = area === 'chat' ? (step === 1 ? 'read' : step >= 2 ? 'write' : null) : (parts.chat ?? null);
  if (conversations !== null) next.timeline = conversations;
  if (chats !== null) next.chat = chats;
  if (documents !== null) next.documents = documents;
  return next;
}

/** The parts of a copy: it holds no write, so whatever is shared is shared to read. */
export function readOnly(parts: ShareParts): ShareParts {
  const next: ShareParts = {};
  for (const area of AREAS) if (stepOf(area, parts) > 0) next[area] = 'read';
  return next;
}

/** The steps an area has: conversations and chats are not shared, read, or read and write; documents add managing them. */
export function stepsOf(area: Area): readonly number[] {
  return area === 'documents' ? [0, 1, 2, 3] : [0, 1, 2];
}

/** Whether a share is a copy that was made, or one that stays. */
export const isCopy = (share: AgentShare): boolean => share.mode === 'snapshot';

/** Whether a share of a place reaches what lies inside it, which is the question the dialog and the tab ask in the same words. */
export const reachOf = (share: AgentShare): Reach => (share.inside ? 'inside' : 'alone');
