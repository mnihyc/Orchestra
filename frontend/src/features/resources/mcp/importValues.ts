/** Stable native discovery identity and the existing resource-name syntax. */
import type { McpFound } from '@/generated/api-types';

export const validMcpName = (name: string): boolean => /^[a-z][a-z0-9_-]{1,31}$/u.test(name);
export const discoveryKey = (found: McpFound): string => JSON.stringify([found.source, found.scope, found.directory, found.name]);
