/** Plain previews use the same expanded mention names as the source message. */
import type { UserRef } from '@/generated/api-types';

export function mentionNames(text: string, users: Iterable<UserRef>): string {
  const known = new Map([...users].map((user) => [user.username.toLowerCase(), user.display_name]));
  return text.replace(/(^|[^\w@])@([\p{L}\p{N}_-]+)/gu, (whole, prefix: string, name: string) => {
    const display = known.get(name.toLowerCase()); return display === undefined ? whole : `${prefix}@${display}`;
  });
}
