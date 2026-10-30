/** A neutral URL for a workbench with no focused content. It never opens a replacement page. */
import { createFileRoute } from '@tanstack/react-router';
export const Route = createFileRoute('/workbench/')({ staticData: { screen: 'page' }, component: () => null });
