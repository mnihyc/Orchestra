/** The blocks a text says, as the page lays them where a person pastes or drops it.
 *
 * Nothing here reads a text: the server's one reader does (`POST /documents/convert`), and what it gives back is
 * turned into these plain values, which `paste.ts` puts on the page.
 */
import type { BlockType, Props } from './blocks.ts';
import type { Inline, NewBlock, TableRow } from '../../generated/api-types.ts';

/** A block of what was read, with the blocks under it. */
export interface Piece {
  type: BlockType;
  props: Props;
  content: Inline[] | TableRow[];
  children: Piece[];
}

/** The blocks the reader made, as pieces. */
export function piecesOf(blocks: readonly NewBlock[]): Piece[] {
  return blocks.map((block) => ({
    type: block.type,
    props: block.props ?? {},
    content: block.content ?? [],
    children: piecesOf(block.children ?? []),
  }));
}
