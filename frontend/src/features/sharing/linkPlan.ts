/** Link gifts use the ordinary native recipient and grant trials; parsing or previewing never grants. */
import { isPlaceKind, previewShare, type AccessTarget } from '@/api/access';
import { ApiError, api, ok } from '@/api/client';
import type { LinkOrigin } from '@/api/linkOrigin';
import { readPersonActions, type PersonActions } from '@/api/personActions';
import { canShareWithAgents } from '@/api/share';
import { PAGE_SIZE } from '@/api/nodes';
import { areasOf, mayGiveParts, partsOfLevel } from '@/features/access/agentShares';
import { parseBlocks, type Block, type Inline } from '@/features/chat/markup';
import { idReferenceOf, referenceOfAddress } from '@/features/people/references';
import type { AccessChoice, Chat, ChatMentionPage, ChatPart, ContentPart, InlineRefKind, NodeRef, SharePreview, SharePreviewAgent, ShareWithAgents, UserRef } from '@/generated/api-types';
import { linkedObject, readOrchLink, type LinkKind } from '@/lib/links';

export type LinkParts = readonly (ChatPart | ContentPart)[];
export interface CarriedLink { id: string; kind?: LinkKind; path?: string; label: string }
export interface LinkContext { origin: LinkOrigin; placeId: string; sender: string | null; agent?: NodeRef; extraAgents?: readonly NodeRef[]; version?: string }
export type AgentGift = ShareWithAgents & { link_origin: LinkOrigin };
export type LinkGift = { key: string; target: AccessTarget } & (
  { kind: 'person'; person: UserRef; choice: AccessChoice; participation: boolean }
  | { kind: 'agent'; agent: NodeRef; share: AgentGift }
);
export interface LinkPlan { gifts: LinkGift[]; people: UserRef[]; agents: NodeRef[]; memberships: AccessTarget[]; unknown: boolean; placeId: string }
export class LinkAudienceChanged extends Error {}

export function carriedLinks(parts: LinkParts): CarriedLink[] {
  const found = new Map<string, CarriedLink>();
  const add = (ref: CarriedLink) => found.set(JSON.stringify([ref.kind, ref.id, ref.path]), ref);
  const links = (runs: readonly Inline[]) => {
    for (const run of runs) {
      if (run.type === 'link') {
        const label = run.children.flatMap((child) => child.type === 'text' ? [child.text] : []).join('');
        const id = idReferenceOf(run.href);
        if (id !== null) add({ ...id, label });
        else {
          try {
            const url = readOrchLink(run.href);
            if (url === null) continue;
            const ref = referenceOfAddress(url.pathname + url.search + url.hash) ?? linkedObject(url.href);
            if (ref !== null) add({ ...ref, label });
          } catch { /* An invalid address carries no grant target. */ }
        }
      } else if (run.type === 'em' || run.type === 'strong' || run.type === 'del') links(run.children);
    }
  };
  const blocks = (items: readonly Block[]) => {
    for (const block of items) {
      if (block.type === 'paragraph') links(block.children);
      else if (block.type === 'quote') blocks(block.children);
      else if (block.type === 'list') block.items.forEach(links);
    }
  };
  for (const part of parts) {
    if (part.type === 'text') blocks(parseBlocks(part.text));
    else if (part.type === 'item_ref') add({ id: part.thread_id, kind: 'thread', label: '' });
  }
  return [...found.values()];
}

const absent = (error: unknown) => error instanceof ApiError && [400, 403, 404, 410].includes(error.status);

async function targetOf(ref: CarriedLink, signal: AbortSignal): Promise<AccessTarget | null> {
  if (ref.kind === 'person' || ref.kind === 'group') return null;
  if (ref.kind !== undefined && ref.kind !== 'node' && ref.kind !== 'file' && ref.kind !== 'environment_file') {
    if (ref.kind === 'org' && !/^[0-9a-f-]{36}$/i.test(ref.id)) {
      try { const org = await ok(api.GET('/orgs/by-slug/{slug}', { params: { path: { slug: ref.id } }, signal })); return { kind: 'org', id: org.id, name: org.name }; }
      catch (error) { if (absent(error)) return null; throw error; }
    }
    return { kind: ref.kind, id: ref.id, name: ref.label || ref.id };
  }
  const kinds: InlineRefKind[] = ref.path !== undefined ? ['environment_file'] : ref.kind === undefined ? ['node', 'document', 'chat', 'file'] : [ref.kind];
  for (const kind of kinds) {
    signal.throwIfAborted();
    try {
      const preview = await ok(api.GET('/references/preview', { params: { query: { kind, id: ref.id, ...(ref.path === undefined ? {} : { path: ref.path }) } }, signal }));
      if (preview.unavailable !== null && preview.unavailable !== undefined) return null;
      if (preview.kind === 'node') return preview.node_kind === null ? null : { kind: preview.node_kind, id: preview.id, name: preview.title };
      if (preview.kind === 'environment_file') return { kind: 'environment', id: preview.id, name: preview.file?.owner_name ?? preview.title };
      if (preview.kind === 'file') {
        const file = preview.file;
        return file !== undefined && file !== null && file.owner_id !== null && (file.owner_kind === 'document' || file.owner_kind === 'chat') ? { kind: file.owner_kind, id: file.owner_id, name: file.owner_name } : null;
      }
      return { kind: preview.kind, id: preview.id, name: preview.title };
    } catch (error) { if (!absent(error)) throw error; }
  }
  return null;
}

export function effectiveBefore(agent: SharePreviewAgent): boolean | null {
  const value = 'effective_before' in agent ? agent.effective_before : null;
  return typeof value === 'object' && value !== null && 'readable' in value && typeof value.readable === 'boolean' ? value.readable : null;
}

export function chatLinkPlace(chat: Chat): string {
  if (chat.node !== null) return chat.node.id;
  const home = 'home' in chat ? chat.home : null;
  return typeof home === 'object' && home !== null && 'id' in home && typeof home.id === 'string' ? home.id : chat.org_id;
}

/** Authorization reads are bounded in parallel, not fanned out once per member. */
async function mapReaders<T, R>(items: readonly T[], read: (item: T) => Promise<R>): Promise<R[]> {
  const result: R[] = new Array<R>(items.length);
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(4, items.length) }, async () => {
    for (;;) { const index = next++; if (index >= items.length) return; result[index] = await read(items[index] as T); }
  }));
  return result;
}

export async function previewLinkPlan(context: LinkContext, refs: readonly CarriedLink[], parts: LinkParts,
  keyFor: (key: string) => string, signal: AbortSignal): Promise<LinkPlan> {
  const plan: LinkPlan = { gifts: [], people: [], agents: [], memberships: [], unknown: false, placeId: context.placeId };
  if (refs.length === 0) return plan;
  const people = new Map<string, UserRef>();
  const agents = new Map<string, NodeRef>();
  const chats = new Map<string, Chat>();
  if (context.origin.kind === 'thread_input') {
    if (context.agent !== undefined) agents.set(context.agent.id, context.agent);
  } else {
    const id = context.origin.chat_id;
    const chat = await ok(api.GET('/chats/{id}', { params: { path: { id } }, signal }));
    chats.set(id, chat); plan.placeId = chatLinkPlace(chat);
    for (const agent of chat.agents ?? []) if (agent.readable) agents.set(agent.thread.id, agent.thread);
    const asked = new Set(context.extraAgents?.map((agent) => agent.id));
    for (const part of parts) if (part.type === 'mention' && part.target.kind === 'agent') asked.add(part.target.thread_id);
    const seen = new Set<string>();
    let cursor: string | null = null;
    do {
      signal.throwIfAborted();
      const page: ChatMentionPage = await ok(api.GET('/chats/{id}/mentions', { params: { path: { id }, query: { q: '', limit: PAGE_SIZE, ...(cursor === null ? {} : { cursor }) } }, signal }));
      for (const choice of page.items) {
        if (choice.available && choice.target.kind === 'user' && choice.user?.kind === 'human' && choice.user.id === choice.target.id && choice.user.id !== context.sender) people.set(choice.user.id, choice.user);
        if (choice.available && choice.target.kind === 'agent' && asked.has(choice.target.thread_id) && choice.thread !== null) agents.set(choice.thread.id, choice.thread);
      }
      cursor = page.next_cursor;
      if (cursor !== null && seen.has(cursor)) throw new LinkAudienceChanged();
      if (cursor !== null) seen.add(cursor);
    } while (cursor !== null);
  }
  const resolved = await mapReaders(refs, (ref) => targetOf(ref, signal));
  const targets = new Map(resolved.flatMap((target) => target === null ? [] : [[`${target.kind}:${target.id}`, target] as const]));
  for (const target of targets.values()) {
    signal.throwIfAborted();
    let group = false;
    if (target.kind === 'chat') {
      let chat = chats.get(target.id);
      if (chat === undefined) { try { chat = await ok(api.GET('/chats/{id}', { params: { path: { id: target.id } }, signal })); } catch (error) { if (absent(error)) continue; throw error; } }
      group = chat.kind === 'group';
      if (chat.name !== null) target.name = chat.name;
    }
    await mapReaders([...people.values()], async (person) => {
      let actions: PersonActions;
      try { actions = await readPersonActions(target, person.id, signal); }
      catch (error) { if (error instanceof ApiError && [403, 404, 410].includes(error.status)) return; throw error; }
      if (actions.can_view === null) { plan.unknown = true; return; }
      if (actions.can_view) return;
      const choice: AccessChoice | null = actions.gives.includes('full_view') ? 'full_view' : group && actions.gives.includes('full_interact') ? 'full_interact' : null;
      if (choice !== null) plan.gifts.push({ kind: 'person', key: JSON.stringify([target.kind, target.id, person.id, choice, context.origin]), target, person, choice, participation: choice === 'full_interact' });
    });
    if (!canShareWithAgents(target.kind)) continue;
    for (const agent of agents.values()) {
      if (target.kind === 'thread' && target.id === agent.id) continue;
      const key = JSON.stringify([target.kind, target.id, agent.id, 'agent-view', context.origin]);
      const share: AgentGift = { client_id: keyFor(key), what: target.kind === 'document' ? { document_id: target.id } : target.kind === 'chat' ? { chat_id: target.id } : { node_id: target.id },
        to: [{ thread_id: agent.id }], mode: 'live', parts: partsOfLevel('view', areasOf(target.kind, 'inside')), inside: isPlaceKind(target.kind),
        download: false, read_first: false, viewers_read: false, link_origin: context.origin };
      let preview: SharePreview;
      try { preview = await previewShare(target, { kind: 'agents', share }, signal); }
      catch (error) { if (error instanceof ApiError && [403, 404, 410].includes(error.status)) { plan.unknown = true; continue; } throw error; }
      if (!preview.allowed) continue;
      const recipient = preview.agents.find((row) => row.to.id === agent.id);
      if (recipient === undefined || effectiveBefore(recipient) === null) { plan.unknown = true; continue; }
      if (effectiveBefore(recipient) === false && mayGiveParts(recipient.already_shared, recipient.parts)) plan.gifts.push({ kind: 'agent', key, target, agent: recipient.to, share });
    }
  }
  plan.gifts.sort((a, b) => a.key.localeCompare(b.key));
  plan.people = [...new Map(plan.gifts.flatMap((gift) => gift.kind === 'person' ? [[gift.person.id, gift.person] as const] : [])).values()];
  plan.agents = [...new Map(plan.gifts.flatMap((gift) => gift.kind === 'agent' ? [[gift.agent.id, gift.agent] as const] : [])).values()];
  plan.memberships = [...new Map(plan.gifts.flatMap((gift) => gift.kind === 'person' && gift.participation ? [[gift.target.id, gift.target] as const] : [])).values()];
  return plan;
}

export const linkPlanSignature = (plan: LinkPlan): string => JSON.stringify([plan.placeId, plan.gifts.map((gift) => gift.key)]);
