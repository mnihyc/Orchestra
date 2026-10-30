/** A place's chat is a row of the finder of its own only when something was said in it; asking for a chat makes no row of an empty one. */
import type { TFunction } from 'i18next';
import { describe, expect, it, vi } from 'vitest';

import { chatWorthARow, type Env, rowOfName } from '@/features/finder/rows';
import type { NameHit } from '@/generated/api-types';

const place = { id: 'topic-1', kind: 'topic', name: 'Fix login redirect', redacted: false } as const;
const hit = (more: Partial<NameHit> = {}): NameHit => ({ id: 'chat-1', kind: 'chat', name: 'Fix login redirect', tag: null, thread_attention: null, color: null, custom_color: null, address: null, archived: false, avatar: null, owner: null, path: [place], recent: false, username: null, ...more });

describe("a place's chat in the finder", () => {
  it('is left out while nothing was said in it, whatever was typed: the place is listed already and opens it', () => {
    expect(chatWorthARow(hit(), () => false)).toBe(false);
  });

  it('is a row once something was said, and where no tree held knows the place', () => {
    expect(chatWorthARow(hit(), () => true)).toBe(true);
    expect(chatWorthARow(hit(), () => null)).toBe(true);
  });

  it('leaves every other name, and direct and group chats, as they are', () => {
    expect(chatWorthARow(hit({ kind: 'topic' }), () => false)).toBe(true);
    expect(chatWorthARow(hit({ path: [] }), () => false)).toBe(true);
  });
});

describe("a place's second act in the finder", () => {
  const env = (): Env => ({ t: ((key: string) => key) as unknown as TFunction, go: vi.fn(), orgSlug: null, exists: () => true, showCard: vi.fn() });

  it('opens its chat with Alt+Enter, and the place itself otherwise', () => {
    const at = env();
    const row = rowOfName(hit({ id: 'topic-1', kind: 'topic', path: [], href: '/topic/topic-1' }), 'go', 'login', at);
    expect(row.beside).toBe(true);
    expect(row.besideAct).toBe('chat');
    row.run('beside');
    expect(at.go).toHaveBeenLastCalledWith({ to: '/topic/topic-1', search: { panel: 'chat' } }, 'beside');
    row.run('here');
    expect(at.go).toHaveBeenLastCalledWith({ to: '/topic/topic-1' }, 'here');
  });

  it('opens a document beside as a document, without turning its second act into a place’s chat', () => {
    const at = env();
    const row = rowOfName(hit({ id: 'doc-1', kind: 'document', path: [], href: '/doc/doc-1' }), 'go', 'plan', at);
    expect(row.beside).toBe(true);
    expect(row.besideAct).toBeUndefined();
    row.run('beside');
    expect(at.go).toHaveBeenCalledWith({ to: '/doc/doc-1' }, 'beside');
  });
});
