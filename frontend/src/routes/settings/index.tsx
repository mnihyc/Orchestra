/** The settings entrance opens the existing Profile page. */
import { createFileRoute, redirect } from '@tanstack/react-router';

export const Route = createFileRoute('/settings/')({
  beforeLoad: () => { throw redirect({ to: '/settings/profile', replace: true }); },
});
