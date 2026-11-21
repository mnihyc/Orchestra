/** One-shot replies started by this document, never credentials kept between documents or readers. */
export interface HtmlReadReceipt {
  response: Response;
  sentAt: number;
  receivedAt: number;
}

export interface HtmlReads {
  scriptsReady: Promise<void>;
  /** The emitted application and styles; development uses the source module directly. */
  application?: { file: string; styles: readonly string[]; initializers: readonly string[] };
  /** The anonymous first shell has painted; feature scripts may now use the remaining connections. */
  painted: () => void;
  take: (request: Request) => Promise<HtmlReadReceipt | null> | null;
  cancel: () => void;
}

/** Only the application client takes this document's initial reads; independent clients have their own transports. */
export function pageHtmlReads(): HtmlReads | undefined {
  return typeof window === 'undefined' ? undefined : (window as Window & { __orchStartupReads?: HtmlReads }).__orchStartupReads;
}
