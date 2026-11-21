import type { ReactNode } from 'react';

/** A row's existing inheritance choice, presented by its outer value control. */
export interface ChoiceInheritance {
  label: string;
  value: ReactNode;
  selected: boolean;
  onSelect: () => void;
}

export function InheritedChoiceLabel({ choice }: { choice: ChoiceInheritance }) {
  return <span className="inline-flex min-w-0 items-baseline gap-2"><span>{choice.label}</span>{' '}<span className="min-w-0 truncate font-normal text-text-muted">{choice.value}</span></span>;
}
