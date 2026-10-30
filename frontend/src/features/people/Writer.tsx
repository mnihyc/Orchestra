/** A block's actual human or agent writer, with the readable originating thread. */
import { Bot } from 'lucide-react';
import { Link } from '@tanstack/react-router';

import type { Author } from '@/generated/api-types';
import { Avatar } from '@/ui/Avatar';

export function Writer({ by }: { by: Author | null }) {
  if (by === null) return null;
  if ('agent' in by) return <span className="inline-flex items-center gap-1"><Bot aria-hidden className="size-4 shrink-0" /><span>{by.agent}</span>{!by.thread.redacted && by.thread.name !== null && <Link to="/thread/$threadId" params={{ threadId: by.thread.id }} className="text-accent-text hover:underline">{by.thread.name}</Link>}</span>;
  return <span className="inline-flex items-center gap-1"><Avatar text={by.avatar.text} color={by.avatar.color} person={by} size={24} /><span>{by.display_name}</span></span>;
}
