/** The thing and part beside a person, for the actions its person card can offer. */
import { createContext, type ReactNode, useContext } from 'react';
import type { AccessKind } from '@/api/access';

export type PersonAnchor = { block: string } | { turn: string } | { message: string };

export interface PersonThing {
  kind: AccessKind;
  id: string;
  anchor?: PersonAnchor | undefined;
}

const ThingContext = createContext<PersonThing | undefined>(undefined);

export function PersonThingProvider({ thing, children }: { thing: PersonThing; children: ReactNode }) {
  return <ThingContext value={thing}>{children}</ThingContext>;
}

export function usePersonThing(): PersonThing | undefined {
  return useContext(ThingContext);
}
