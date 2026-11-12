/** Complete generated file metadata for workspace reader and writer controls. */
import type { FileStat, GitStatus } from '@/generated/api-types';
import { AT } from './thread-rows';

export function workspaceFile(path: string, text: string, more: Partial<FileStat> = {}): FileStat {
  return { cached: true, hash: 'observed-version', is_text: true, link_target: null, media_type: 'text/plain', mode: 0o644,
    mtime: AT, name: path.split('/').at(-1) ?? path, path, size: new TextEncoder().encode(text).length, type: 'file', ...more };
}

export function workspaceGit(more: Partial<GitStatus> = {}): GitStatus {
  return { available: true, reason: null, branch: 'main', head: 'a'.repeat(40), upstream: null, ahead: 0, behind: 0,
    repository_dir: '', status_hash: 'b'.repeat(64), remotes: [], changes: [], ...more };
}
