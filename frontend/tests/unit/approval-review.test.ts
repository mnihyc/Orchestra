import { describe, expect, it } from 'vitest';
import { approvalReview } from '@/features/timeline/tools/ApprovalReview';

describe('native permission reviews', () => {
  it('shows the reason, outcome and actual model without turning a denial into a card', () => {
    expect(approvalReview({ model: 'codex-auto-review', reported: true, outcome: 'denied', reason: 'Private data would leave the workspace', action: { command: 'upload' }, usage: null })).toMatchObject({ model: 'codex-auto-review', reported: true, outcome: 'denied', reason: 'Private data would leave the workspace' });
  });
  it('distinguishes a declared default from a reported model and rejects malformed facts', () => {
    expect(approvalReview({ model: 'claude-opus-5', outcome: 'denied' })?.reported).toBe(false);
    expect(approvalReview({ outcome: 'approved' })).toBeNull();
  });
});
