/** A failed turn resolves its original paste through admitted point references, never a history walk. */
import type { ContentPart, Item, ThreadInput } from '@/generated/api-types';
export function turnPasteSource(
  inputs: ReadonlyMap<string, ThreadInput>, items: ReadonlyMap<string, Item>,
  inputId: string, inputItemId: string | undefined,
): readonly ContentPart[] | undefined {
  const input = inputs.get(inputId);
  if (input !== undefined) return input.content;
  const item = inputItemId === undefined ? undefined : items.get(inputItemId);
  return item?.kind === 'user_message' ? item.payload.parts : undefined;
}
