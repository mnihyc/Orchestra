/** The node's ordinary chat entrance resolves to the chat's stable content address. */
import { createFileRoute, redirect } from '@tanstack/react-router';
import { placeChatAddress } from '@/features/chat/targets';
import { SkeletonRows } from '@/ui/Skeleton';

export const Route = createFileRoute('/place-chat/$nodeId/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { message?: string } => typeof search.message === 'string' && search.message !== '' ? { message: search.message } : {},
  beforeLoad: async ({ params, search }) => { throw redirect({ href: await placeChatAddress(params.nodeId, search.message), replace: true }); },
  component: () => <SkeletonRows rows={4} />,
});
