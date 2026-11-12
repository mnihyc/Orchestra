/** Follow intervals for visible work; each supplier keeps its own stop conditions and server delays. */

/** Cleanup and resource activity share the existing one-second job-progress cadence. */
export const DEFAULT_CLEANUP_POLL_MS = 1_000;
export const DEFAULT_RESOURCE_IMPACT_POLL_MS = 1_000;
/** Discussion notices are followed only while an observed job is queued or running. */
export const DEFAULT_DOCUMENT_NOTICE_POLL_MS = 2_000;
/** A fetched program build takes seconds to check; lists stop following when checking ends. */
export const DEFAULT_PROGRAM_BUILD_POLL_MS = 5_000;
/** Used only until the edge status or operator supplies its own installation follow interval. */
export const DEFAULT_INSTALLATION_POLL_SECONDS = 1;
