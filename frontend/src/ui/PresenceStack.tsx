/** Shared PresenceStack component (people); primitives are wrapped here.
 *
 * Who is looking at a thing now: up to four avatars, then "+n"; the names are under the pointer and
 * read by a screen reader.
 */
import { useTranslation } from 'react-i18next';

import type { PresenceUser } from '@/generated/api-types';
import { AvatarStack } from '@/ui/AvatarStack';
import { Tooltip } from '@/ui/Tooltip';

/** The most avatars shown before the rest are counted. */
export const PRESENCE_SHOWN = 4;

export interface PresenceStackProps {
  users: readonly PresenceUser[];
  /** The person who is looking at this, left out of the list: they know. */
  without?: string | null;
  className?: string;
}

export function PresenceStack({ users, without = null, className }: PresenceStackProps) {
  const { t } = useTranslation();
  const others = users.filter((entry) => entry.user.id !== without);
  if (others.length === 0) return null;
  const names = others.map((entry) => entry.user.display_name).join(', ');
  const said = t('ui.presence.here', { names });
  return (
    <Tooltip content={said}>
      <span role="group" aria-label={said}>
        <AvatarStack users={others.map((entry) => entry.user)} max={PRESENCE_SHOWN} {...(className === undefined ? {} : { className })} />
      </span>
    </Tooltip>
  );
}
