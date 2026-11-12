/** A summary view is supplied by its ordinary authorized page reader. */
import type { ResourceSubject } from '@/features/resource-facts/facts';

export interface SummaryTarget {
  kind: 'thread' | 'topic' | 'document' | 'chat' | 'chat_title' | 'help' | 'search_results';
  /** A canonical readable page or exact result-selection context. */
  reference: string;
}

/** Co-mounted catch-up and title controls name their actual subject. */
export function summaryActionKey(kind: SummaryTarget['kind'], action: 'ask' | 'refresh' | 'choose' | 'details') {
  const subject = kind === 'chat' ? 'catchUpActions' : kind === 'chat_title' ? 'chatTitleActions' : null;
  return subject === null ? `summaryBox.${action}` : `summaryBox.${subject}.${action}`;
}

export type SummaryState = 'empty' | 'fresh' | 'stale' | 'scheduled' | 'updating' | 'failed' | 'no_summarizer';

export type SummaryProducer =
  | { kind: 'written' }
  | Extract<ResourceSubject, { kind: 'model' }>
  | { kind: 'extension'; name: string };

export interface SummarySnapshot {
  /** Changes with the supplied text or durable state, including failed attempts. */
  revision: string;
  state: SummaryState;
  text: string | null;
  /** Only facts the current reader may see; null reveals no hidden producer. */
  producer: SummaryProducer | null;
  updated_at: string | null;
  /** Actual native waiting reason/eligibility, independent of the previous paragraph's age. */
  wait?: { reason: 'interval' | 'recovery' | 'queue'; until: string | null } | null;
  /** The ordinary request error, rendered by the shared error component. */
  error?: unknown;
}

export interface SummaryAskContext {
  target: SummaryTarget;
  summary: string | null;
}
