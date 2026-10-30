/** The resource landing lists each kind, its visible count and its creation entrance. */
import { createFileRoute } from '@tanstack/react-router';
import { ResourceLanding } from '@/features/resources/Landing';

export const Route = createFileRoute('/resources/')({ staticData: { screen: 'page' }, component: ResourceLanding });
