/** The shared personal feed owns notification projections; a second listener must not refresh them all. */
import { useUserFeed } from '@/realtime/live';

/** Keep the one personal stream followed without loading the Inbox on a settings screen. */
export function useAwayLive(): void {
  useUserFeed(false);
}
