import { describe, expect, it } from 'vitest';
import { reviewOutcome } from '@/features/timeline/tools/reviewFacts';
import { i18n } from '@/i18n';
import { approvalReview } from '@/features/timeline/tools/ApprovalReview';

describe('native permission reviews', () => {
  it('shows the reason, outcome and actual model without inventing a different outcome', () => {
    expect(approvalReview({ model: 'codex-auto-review', reported: true, outcome: 'denied', reason: 'Private data would leave the workspace', action: { command: 'upload' }, usage: null })).toMatchObject({ model: 'codex-auto-review', reported: true, outcome: 'denied', reason: 'Private data would leave the workspace' });
  });
  it('distinguishes a declared default from a reported model and rejects malformed facts', () => {
    expect(approvalReview({ model: 'claude-opus-5', outcome: 'denied' })?.reported).toBe(false);
    expect(approvalReview({ outcome: 'approved' })).toBeNull();
  });
});

for(const locale of ['en','zh-CN'] as const)it(`localizes every known native review outcome (${locale})`,()=>{
  const t=i18n.getFixedT(locale);
  for(const [status,key] of [['inProgress','reviewing'],['approved','approved'],['denied','denied'],['timedOut','timedOut'],['aborted','aborted'],['failed','failed']] as const){
    expect(reviewOutcome(status,t)).toBe(t(`timeline.permissionReview.status.${key}`));
  }
  expect(reviewOutcome('native-future-state',t)).toContain('native-future-state');
});
