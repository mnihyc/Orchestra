/** New document creates the page immediately; its own title field is where its name is written. */
import type { ReactNode } from 'react';
import { ActionTrigger } from '@/ui/ActionTrigger';

export interface NameNewDocumentProps {
  /** The existing New document control, retaining its permission and pending treatment. */
  trigger: ReactNode;
  /** Whether the document is being made. */
  pending: boolean;
  /** Creates the page without a preliminary name. */
  onMake: (title: string | undefined) => void;
  align?: 'start' | 'center' | 'end';
}

export function NameNewDocument({ trigger, pending, onMake }: NameNewDocumentProps) {
  return <ActionTrigger pending={pending} onAction={() => onMake(undefined)}>{trigger}</ActionTrigger>;
}
