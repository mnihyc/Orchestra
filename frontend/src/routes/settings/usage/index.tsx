/** The former settings address opens the actual Usage report, without an intermediate link-only page. */
import { createFileRoute, redirect } from '@tanstack/react-router';

export const Route = createFileRoute('/settings/usage/')({
  staticData: { screen: 'page' },
  beforeLoad: () => { throw redirect({ to: '/usage', replace: true }); },
});
