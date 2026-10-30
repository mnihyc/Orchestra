/** Which object streams the server has admitted on this connection; this records delivery, never authority. */
const following = new Set<string>();

export function resourceFollowing(kind: string, id: string): boolean {
  return following.has(`${kind}:${id}`);
}

export function setResourceFollowing(kind: string, id: string, active: boolean): void {
  if (active) following.add(`${kind}:${id}`);
  else following.delete(`${kind}:${id}`);
}

export function clearResourceFollowing(): void {
  following.clear();
}
