/** Management without a section: the overview. */
import { createFileRoute, redirect } from '@tanstack/react-router';

export const Route = createFileRoute('/admin/')({
  staticData: { screen: 'page' },
  beforeLoad: () => {
    throw redirect({ to: '/admin/$section', params: { section: 'overview' }, replace: true });
  },
});
