/** Existing localized plan sentences retain their words while known resource names become inspection chips. */
import type { ReactNode } from 'react';
import { ResourceChip } from './ResourceFacts';
import type { ResourceSubject } from './facts';
import type { UserRef } from '@/generated/api-types';
import { UserChip } from '@/ui/UserChip';

export interface NamedResource { label: string; subject: ResourceSubject }

function nameAt(text: string, label: string, from: number): number {
  for (let at = text.indexOf(label, from); at >= 0; at = text.indexOf(label, at + label.length)) {
    // Latin names must not turn part of an ordinary word into a resource. Other scripts can
    // adjoin a name grammatically without spaces, so they retain their localized sentence.
    const before = text[at - 1] ?? '';
    const after = text[at + label.length] ?? '';
    if (!(/[A-Za-z0-9_]/.test(label[0] ?? '') && /[A-Za-z0-9_]/.test(before))
      && !(/[A-Za-z0-9_]/.test(label.at(-1) ?? '') && /[A-Za-z0-9_]/.test(after))) return at;
  }
  return -1;
}

export function ResourceSentence({ text, resources = [], people = [], card = true }: { text: string; resources?: readonly NamedResource[] | undefined; people?: readonly UserRef[] | undefined; card?: boolean }) {
  const parts: ReactNode[] = [];
  type Name = { kind: 'resource'; label: string; value: NamedResource } | { kind: 'person'; label: string; value: UserRef };
  const pending: Name[] = [...resources.map((value): Name => ({ kind: 'resource', label: value.label, value })),
    ...people.map((value): Name => ({ kind: 'person', label: value.display_name, value }))].filter((value) => value.label !== '');
  let from = 0;
  while (pending.length > 0) {
    const matches = pending.map((resource, index) => ({ resource, index, at: nameAt(text, resource.label, from) }))
      .filter((match) => match.at >= 0).sort((a, b) => a.at - b.at || b.resource.label.length - a.resource.label.length);
    const first = matches[0];
    if (first === undefined) break;
    parts.push(text.slice(from, first.at));
    parts.push(first.resource.kind === 'person'
      ? <UserChip key={`${first.at}:person`} user={first.resource.value} />
      : <ResourceChip key={`${first.at}:${first.resource.value.subject.kind}`} subject={first.resource.value.subject} card={card} hint={false}>{first.resource.label}</ResourceChip>);
    from = first.at + first.resource.label.length;
    pending.splice(first.index, 1);
  }
  parts.push(text.slice(from));
  return parts;
}
