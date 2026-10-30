/** The signed-in person's words, pins and privacy on their own page. */
import { createFileRoute } from '@tanstack/react-router';
import { ProfileSettingsPage } from '@/features/profile/Settings';

export const Route = createFileRoute('/settings/your-page/')({ staticData: { screen: 'page' }, component: ProfileSettingsPage });
