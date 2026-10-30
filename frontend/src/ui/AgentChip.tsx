/** A known agent's name opens its own card wherever it stands outside another control. */
import { useLayoutEffect, useRef, useState } from 'react';
import { AgentCard, type AgentIdentity } from '@/ui/AgentCard';
import { standsAlone } from '@/ui/pressed';

export function AgentChip(identity: AgentIdentity) {
  const at = useRef<HTMLSpanElement>(null);
  const [alone, setAlone] = useState(false);
  useLayoutEffect(() => { setAlone(standsAlone(at.current)); }, []);
  const name = <span ref={at} title={identity.name} className="min-w-0 truncate font-medium">{identity.name}</span>;
  return alone ? <AgentCard {...identity}>{name}</AgentCard> : name;
}
