/** The terms editor opens the same native members composer, with only its subject prefilled. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { MessageComposer } from './Composer';

export function TellMembers({ id }: { id: string }) {
  const { t } = useTranslation();
  const node = useQuery(nodeQuery(id));
  const [open, setOpen] = useState(false);
  const org = node.data?.kind === 'org' ? node.data : undefined;
  if (org === undefined || !holds(org.access, 'members.manage')) return null;
  return <><Button size="sm" variant="ghost" onClick={() => setOpen(true)}>{t('adminMessages.tellMembers')}</Button>
    {open && <MessageComposer scope={{ kind: 'org', id: org.id, name: org.name }} initialTitle={t('adminMessages.termsChanged', { org: org.name })} onClose={() => setOpen(false)} />}
  </>;
}
