/** An accepted administrator message, read through its native recipient or current sender gate. */
import { createFileRoute } from '@tanstack/react-router';
import { AdministratorMessage } from '@/features/admin-messages/Message';
import { usePageParams } from '@/features/workbench/pageContext';

export const Route = createFileRoute('/messages/$id/')({
  staticData: { screen: 'page' },
  component: MessageRoute,
});
function MessageRoute() {
  const { id } = usePageParams(Route);
  return <AdministratorMessage id={id} />;
}
