/** Personal notifications. Loader, permission handling and search schema are pending.
 *
 * What a person is told and how: the levels of a place, approvals, chats, quiet hours, and the ways out of the
 * application, each off until they switch it on. The screen is built in `features/settings/personal`.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Notifications } from '@/features/settings/personal/Notifications';

export const Route = createFileRoute('/settings/notifications/')({
  staticData: { screen: 'page' },
  component: Notifications,
});
