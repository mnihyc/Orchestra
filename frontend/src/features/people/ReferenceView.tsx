/** A document reference's current readable name and kind, rendered as an inline link without changing its stored id; it opens the reference's card, at its part. */
import { NodeViewWrapper, type NodeViewProps } from '@tiptap/react';

import { TypedChip } from '@/features/people/NamedChip';
import { type TypedReference } from '@/features/people/references';
import type { RefPart } from '@/generated/api-types';
import { isChipSize, type ChipPresentation } from './chipSizes';

/** The reference a chip of the editor names: the thing, and the part where it has one of its kind's. */
function referenceOf(kind: unknown, id: string, part: RefPart | null, path: unknown): TypedReference {
  if (kind === 'file' || kind === 'environment_file') return { kind, id, ...(typeof path === 'string' ? { path } : {}), ...(part !== null && 'line' in part ? { line: part.line } : {}) };
  if (kind === 'document') return { kind: 'document', id, ...(part !== null && 'block' in part ? { block: part.block } : {}) };
  if (kind === 'chat') return { kind: 'chat', id, ...(part !== null && 'message' in part ? { message: part.message } : {}) };
  return { kind: 'node', id, ...(part !== null && 'turn' in part ? { turn: part.turn } : {}) };
}

export function ReferenceView({ node, editor, getPos, updateAttributes }: NodeViewProps) {
  const id = String(node.attrs.id ?? '');
  const reference = referenceOf(node.attrs.kind, id, (node.attrs.part ?? null) as RefPart | null, node.attrs.path);
  if (typeof node.attrs.label === 'string') reference.label = node.attrs.label;
  const position = getPos();
  const resolved = position === undefined ? null : editor.state.doc.resolve(position);
  const alone = resolved?.parent.childCount === 1;
  const inTable = resolved !== null && Array.from({ length: resolved.depth }, (_, index) => resolved.node(index + 1).type.name).some((name) => name === 'tableCell' || name === 'tableHeader');
  const presentation: ChipPresentation = { document: true, place: inTable ? 'compact' : alone ? 'document' : 'prose',
    size: isChipSize(node.attrs.display) ? node.attrs.display : undefined,
    onSizeChange: editor.isEditable ? (display) => updateAttributes({ display }) : undefined };
  return <NodeViewWrapper as="span" className="oi oi-ref" contentEditable={false} data-ref={id}>
    <TypedChip reference={reference} presentation={presentation} />
  </NodeViewWrapper>;
}
