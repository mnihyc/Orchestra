/** Workspace paths carry their source environment; a drop cannot reinterpret them elsewhere. */
export function workspaceDrag(environmentId: string, paths: readonly string[]): string {
  return JSON.stringify({ environmentId, paths });
}
export function readWorkspaceDrag(raw: string, environmentId: string): { kind: 'local'; paths: string[] } | { kind: 'foreign' | 'invalid' } {
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== 'object' || value === null || !('environmentId' in value) || typeof value.environmentId !== 'string'
      || !('paths' in value) || !Array.isArray(value.paths) || !value.paths.every((path): path is string => typeof path === 'string')) return { kind: 'invalid' };
    return value.environmentId === environmentId ? { kind: 'local', paths: value.paths } : { kind: 'foreign' };
  } catch { return { kind: 'invalid' }; }
}
