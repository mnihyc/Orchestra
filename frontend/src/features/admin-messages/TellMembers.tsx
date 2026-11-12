/** The terms editor opens the same native members composer, with only its subject prefilled. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { MessageComposer, type MessageScope } from './Composer';

export function TellMembers({ id }: { id: string }) {
  const { t } = useTranslation();
  const node = useQuery(nodeQuery(id));
  const [opened, setOpened] = useState<MessageScope & { kind: 'org' } | null>(null);
  const org = node.data?.kind === 'org' ? node.data : undefined;
  const reason = node.isPending ? t('common.loading') : node.isError || org === undefined ? t('implicitAvailability.orgUnavailable')
    : !holds(org.access, 'members.manage') ? t('implicitAvailability.membersManage') : undefined;
  return <><Button size="sm" variant="ghost" blocked={reason !== undefined} reason={reason} onClick={() => { if (org !== undefined) setOpened({ kind: 'org', id: org.id, name: org.name }); }}>{t('adminMessages.tellMembers')}</Button>
    {node.isError && <Button size="sm" variant="ghost" pending={node.isFetching} onClick={() => void node.refetch()}>{t('common.tryAgain')}</Button>}
    {opened !== null && <MessageComposer scope={opened} unavailableReason={opened.id === org?.id ? reason : t('implicitAvailability.orgUnavailable')} initialTitle={t('adminMessages.termsChanged', { org: opened.name })} onClose={() => setOpened(null)} />}
  </>;
}
