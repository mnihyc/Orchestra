/** Optional normalized web actions remain readable alongside legacy search rows. */
interface ActionView {
  type: 'search' | 'open_page' | 'find_in_page' | 'other';
  url: string | null;
  pattern: string | null;
}

export function webAction(tool: unknown): ActionView {
  // The optional field may be absent on old persisted rows or a newer kind may reach an older client.
  const action = typeof tool === 'object' && tool !== null && 'action' in tool ? tool.action : null;
  if (action === null || action === undefined) return { type: 'search', url: null, pattern: null };
  if (typeof action !== 'object') return { type: 'other', url: null, pattern: null };
  const reported = 'type' in action ? action.type : null;
  const type = reported === 'search' || reported === 'open_page' || reported === 'find_in_page' ? reported : 'other';
  const url = 'url' in action && typeof action.url === 'string' && action.url !== '' ? action.url : null;
  const pattern = 'pattern' in action && typeof action.pattern === 'string' && action.pattern !== '' ? action.pattern : null;
  return { type, url, pattern };
}
