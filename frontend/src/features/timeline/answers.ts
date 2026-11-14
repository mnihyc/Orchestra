/** The same recorded positions decide which message is a turn's answer in the conversation and its outline. */
export interface AnswerFacts {
  ended: boolean;
  lastAgent: number | null;
  lastFinal: number | null;
  lastWork: number | null;
}

export function isTurnAnswer(seq: number, phase: string | null | undefined, facts: AnswerFacts): boolean {
  if (!facts.ended) return false;
  if (facts.lastFinal !== null) return seq === facts.lastFinal;
  return (phase === null || phase === undefined) && seq === facts.lastAgent && seq > (facts.lastWork ?? -1);
}
