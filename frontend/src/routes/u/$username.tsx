/** A person's page, opened by their public username. */
import { usePageParams } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { ProfilePage } from '@/features/profile/Page';

export const Route = createFileRoute('/u/$username')({ staticData: { screen: 'page' }, component: PersonPage });
function PersonPage() { const { username } = usePageParams(Route); return <ProfilePage username={username} />; }
