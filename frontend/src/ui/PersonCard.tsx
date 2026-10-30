/** A person's known identity with the current screen's authorized actions, opened by pointer or keyboard.
 *
 * It shows under the pointer and under the keyboard's focus alike, and a click (or Enter) keeps it open
 * until it is dismissed, the passing one giving way to it. A click on the name opens the card and
 * nothing that the name stands in. */
import { type ReactNode, type Ref, useEffect, useState } from 'react';
import type { UserRef } from '@/generated/api-types';
import { CardDetails } from '@/features/profile/CardDetails';
import { PersonQuickCard } from '@/features/people/PersonQuickCard';
import { type PersonThing, usePersonThing } from '@/features/people/personContext';
import { UserChip } from '@/ui/UserChip';
import { DetailCard, type CardController } from '@/ui/DetailCard';

export function PersonCard({ user, actions, children, thing, label, choice = false, active = false, controller, returnFocus }: { user: UserRef; actions?: ReactNode; children: ReactNode; thing?: PersonThing | undefined; label?: string | undefined; choice?: boolean; active?: boolean; controller?: Ref<CardController> | undefined; returnFocus?: (() => void) | undefined }) {
  const inherited = usePersonThing();
  const context = thing ?? inherited;
  const [touch, setTouch] = useState(false);
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia('(hover: none), (pointer: coarse)');
    const update = () => setTouch(media.matches);
    update(); media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  const content = user.kind === 'human' && user.username !== '' ? <CardDetails username={user.username} actions={actions} hideMessage={touch} /> : <div className="flex max-w-[min(20rem,calc(100vw-2rem))] flex-col gap-3 p-3 text-text"><UserChip user={user} size={40} showUsername={user.username !== ''} card={false} />{actions}</div>;
  return <DetailCard label={label ?? user.display_name} asChild={choice} choice={choice} avoidChoiceList={choice} active={choice && touch ? false : active} controller={controller} returnFocus={returnFocus} passingContent={() => <PersonQuickCard user={user} thing={context} />}
    content={() => <>{touch && <PersonQuickCard user={user} thing={context} identity={false} />}{content}</>} className="hover:underline">{children}</DetailCard>;
}
