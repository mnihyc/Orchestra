/** Draft/edit text carries deliberate targets once, at the position of their visible current label. */
import type { ChatMentionName, ChatMentionTarget, ChatPart } from '../../generated/api-types.ts';
import { type KnownPlaces, placeLabel } from '../../lib/placeLabel.ts';

/** A role token keeps its identity while its place uses the one shared readable-place label. */
export function labelPlaceMention(target: ChatMentionTarget, current: string, known: KnownPlaces, words: (role: Extract<ChatMentionTarget,{kind:'role'}>['role'],place:string) => string): string {
  if (target.kind !== 'role') return current;
  const place = [...known.values()].flat().find((place) => place.id === target.node_id);
  return place === undefined ? current : words(target.role,placeLabel(place,known));
}

export function typedTextParts(source: string, mentions: readonly ChatMentionName[]): ChatPart[] {
  const text = source.trim();
  const named = mentions.filter((mention) => mention.name !== '');
  const parts: ChatPart[] = [];
  const occurrences = new Map<string, number>();
  let at = 0;
  while (at < text.length) {
    const next = named.flatMap((mention) => {
      let index = text.indexOf(`@${mention.name}`, at);
      while (index >= 0) {
        const end = index + mention.name.length + 1;
        const before = index === 0 ? '' : text[index - 1] ?? '';
        const after = text[end] ?? '';
        if ((before === '' || /\s/.test(before)) && !/[\p{L}\p{N}_-]/u.test(after)) return [{ mention, index, end }];
        index = text.indexOf(`@${mention.name}`, index + 1);
      }
      return [];
    }).sort((one, two) => one.index - two.index || two.end - one.end)[0];
    if (next === undefined) { parts.push({ type: 'text', text: text.slice(at) }); break; }
    if (next.index > at) parts.push({ type: 'text', text: text.slice(at, next.index) });
    const same = named.filter((mention) => mention.name === next.mention.name);
    const used = occurrences.get(next.mention.name) ?? 0;
    const identity = same[Math.min(used, same.length - 1)] ?? next.mention;
    parts.push({ type: 'mention', target: identity.target });
    occurrences.set(next.mention.name, used + 1);
    at = next.end;
  }
  return parts;
}

/** A text edit keeps the identities of untouched tokens, including two threads with equal names. */
export function editedMentionNames(before: string, after: string, names: readonly ChatMentionName[]): ChatMentionName[] {
  let left = 0;
  while (left < before.length && left < after.length && before[left] === after[left]) left += 1;
  let right = 0;
  while (right < before.length - left && right < after.length - left && before[before.length - right - 1] === after[after.length - right - 1]) right += 1;
  const end = before.length - right;
  let at = before.length - before.trimStart().length;
  const kept: ChatMentionName[] = [];
  for (const part of typedTextParts(before, names)) {
    if (part.type === 'text') { at += part.text.length; continue; }
    if (part.type !== 'mention') continue;
    const name = names.find((name) => JSON.stringify(name.target) === JSON.stringify(part.target));
    if (name === undefined) continue;
    const last = at + name.name.length + 1;
    if (end === left ? left <= at || left >= last : last <= left || at >= end) kept.push(name);
    at = last;
  }
  return typedTextParts(after, kept).flatMap((part) => {
    if (part.type !== 'mention') return [];
    const found = kept.find((name) => JSON.stringify(name.target) === JSON.stringify(part.target));
    return found === undefined ? [] : [found];
  });
}

/** A picker inserts this token's identity at the same-name occurrence that was actually chosen. */
export function insertedMentionName(text: string, at: number, mention: ChatMentionName, names: readonly ChatMentionName[]): ChatMentionName[] {
  const same = names.filter((name) => name.name === mention.name);
  const earlier = typedTextParts(text.slice(0, at), same).filter((part) => part.type === 'mention').length;
  return [...names.filter((name) => name.name !== mention.name), ...same.slice(0, earlier), mention, ...same.slice(earlier)];
}

/** Stored targets reopen with current safe names, never raw group/role identifiers. */
export function draftMentionText(parts: readonly ChatPart[], mentions: readonly ChatMentionName[]): string {
  const hasTargets = parts.some((part) => part.type === 'mention');
  return parts.flatMap((part) => {
    if (part.type === 'text') return [part.text];
    if (part.type !== 'mention') return [];
    const found = mentions.find((mention) => JSON.stringify(mention.target) === JSON.stringify(part.target));
    return [`@${found?.name ?? 'mention'}`];
  }).join(hasTargets ? '' : '\n\n');
}
