/** A document mention is the same person chip used elsewhere. */
import { NodeViewWrapper, type NodeViewProps } from '@tiptap/react';
import { useState } from 'react';

import { MentionPrompt } from '@/features/people/MentionPrompt';
import type { PersonThing } from '@/features/people/personContext';
import type { UserRef } from '@/generated/api-types';
import { UserChip } from '@/ui/UserChip';

export function MentionView({ node, editor, getPos }: NodeViewProps) {
  const user = node.attrs.user as UserRef | null | undefined;
  const [prompt, setPrompt] = useState(node.attrs.prompt === true);
  const position = getPos();
  const resolved = position === undefined ? null : editor.state.doc.resolve(position);
  let block: string | undefined;
  if (resolved !== null) {
    for (let depth = resolved.depth; depth >= 0; depth -= 1) {
      const parent = resolved.node(depth);
      if ((parent.type.name === 'block' || parent.type.name === 'leafBlock') && typeof parent.attrs.id === 'string') { block = parent.attrs.id; break; }
    }
  }
  const doc = (editor.storage as unknown as { orchDoc?: { id?: string } }).orchDoc;
  const thing: PersonThing | undefined = doc?.id === undefined ? undefined : { kind: 'document', id: doc.id, ...(block === undefined ? {} : { anchor: { block } }) };
  const chip = user === null || user === undefined
    ? <span>@{String(node.attrs.name ?? node.attrs.username ?? '')}</span>
    : <UserChip user={user} mention size={24} thing={thing} className="align-baseline" />;
  return <NodeViewWrapper as="span" className="oi oi-mention" contentEditable={false} data-mention={String(node.attrs.user_id ?? '')}>
    {prompt && thing !== undefined && user !== null && user !== undefined
      ? <MentionPrompt user={user} thing={thing} canView={node.attrs.can_view === true} onClose={() => setPrompt(false)}>{chip}</MentionPrompt>
      : chip}
  </NodeViewWrapper>;
}
