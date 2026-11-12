# Summary paragraph

SummaryBox renders an authorized summary snapshot with the shared status, resource, time and action components. It owns only the pending state of an explicit refresh and its local error. It performs no request on mount and does not choose a route, provider, payer, catch-up cursor or freshness interval. Its page adapter supplies the snapshot and current permitted callbacks.

Text remains visible during refresh and failure. Written help has no refresh action. Model producers use ResourceChip; a hidden producer stays absent. Ask receives the canonical page context and the current paragraph, including null when nothing has been summarized, without sending a question automatically. A non-secret reader/view key and the page reference isolate pending state across readers and pages. The page reader remains responsible for removing content when its authority is lost.
