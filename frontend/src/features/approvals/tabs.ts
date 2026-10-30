/** The tabs of Approvals, as the address names them: read by its route before the page itself is fetched. */
export type ApprovalsTab = 'waiting' | 'requested' | 'history';

export const APPROVAL_TABS: readonly ApprovalsTab[] = ['waiting', 'requested', 'history'];
