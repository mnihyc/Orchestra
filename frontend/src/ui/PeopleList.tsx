/** Avatar stacks and operation counts share one bounded list and each person's own card. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { UserRef } from '@/generated/api-types';
import { IntentCard } from '@/ui/IntentCard';
import { UserChip } from '@/ui/UserChip';

export type ListedPerson=Pick<UserRef,'avatar'|'display_name'|'id'|'kind'>&{username?:string;address?:string|null};
export function PeopleList({users,total=users.length,hidden,children}:{users:readonly ListedPerson[];total?:number;hidden?:number;children?:ReactNode}) {
  const {t}=useTranslation();
  const more=hidden??Math.max(0,total-users.length);
  return <div className="max-h-[min(22rem,var(--radix-popper-available-height,22rem))] w-[18rem] max-w-full overflow-y-auto"><ul className="flex min-w-0 flex-col gap-2">{users.map((user)=><li key={user.id} className="flex min-h-(--control-sm) min-w-0"><UserChip user={{...user,username:user.username??user.address?.replace(/^@/,'')??''}} size={24}/></li>)}</ul>{more>0&&<p className="mt-2 text-sm text-text-muted">{t('ui.avatarStack.andMore',{count:more})}</p>}{children}</div>;
}
export function PeopleCount({users,total,label}:{users:readonly ListedPerson[];total:number;label:string}) {
  const {t}=useTranslation();
  return <IntentCard label={t('ui.avatarStack.list')} trigger={<button type="button" className="min-w-0 text-sm underline decoration-dotted underline-offset-4">{label}</button>}><PeopleList users={users} total={total}/></IntentCard>;
}
export { IntentCard as PeopleCard } from '@/ui/IntentCard';
