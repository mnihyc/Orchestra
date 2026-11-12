/** Conversation preferences follow the person through the existing timeline state object. */
export function timelineState(state: Record<string, unknown> | undefined): Record<string, unknown> {
  const value = state?.timeline;
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? { ...value } : {};
}

export function linkSharingPreference(state: Record<string, unknown> | undefined): boolean | null {
  const value = state?.timeline;
  if (value !== undefined && value !== null && (typeof value !== 'object' || Array.isArray(value))) return null;
  const choice = timelineState(state).share_on_link;
  return choice === undefined ? true : typeof choice === 'boolean' ? choice : null;
}
