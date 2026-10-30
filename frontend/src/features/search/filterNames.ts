/** Keep labels only for the fixed set of selected filters and the newest choice. */
import type { SearchParams } from './params';

export function rememberedFilterNames(before: Readonly<Record<string, string>>, params: SearchParams, picked: { id: string; name: string }): Record<string, string> {
  const names: Record<string, string> = {};
  for (const id of [params.scope, params.document_id, params.chat_id, params.owner, params.created_by, params.edited_by, params.author, picked.id]) {
    if (id === undefined) continue;
    const name = id === picked.id ? picked.name : before[id];
    if (name !== undefined) names[id] = name;
  }
  return names;
}
