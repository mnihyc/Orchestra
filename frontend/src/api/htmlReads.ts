/** One-shot replies started by this document, never credentials kept between documents or readers. */
export interface HtmlReadReceipt {
  response: Response;
  sentAt: number;
  receivedAt: number;
}

export interface HtmlReads {
  scriptsReady: Promise<void>;
  /** The edge supplied an anonymous metadata response with this sign-in document. */
  readonly hasLoginMeta?: boolean;
  /** The emitted application and styles; development uses the source module directly. */
  application?: { file: string; styles: readonly string[]; initializers: readonly string[] };
  /** The anonymous first shell has painted; feature scripts may now use the remaining connections. */
  painted: () => void;
  /** The addressed screen has committed, or navigation has replaced it. Release its background shell read. */
  contentShown?: () => void;
  afterThreadContent?: Promise<void>;
  /** Settles when the document's first content, of whatever page, is on the screen; absent after that. */
  afterFirstContent?: Promise<void>;
  /** The address this document opened, by which its first reads are kept for the next document of it. */
  readonly openingAddress?: string;
  /** The first content is on the screen: what only accompanies it may start. */
  firstContent?: () => void;
  /** The opening's reads have all been asked: an answer nobody took is not lent to a later read. */
  openingDone?: () => void;
  take: (request: Request) => Promise<HtmlReadReceipt | null> | null;
  /** Retire the addressed page's read while the shared shell continues loading. */
  visit: (pathname: string) => void;
  cancel: () => void;
}

/** Only the application client takes this document's initial reads; independent clients have their own transports. */
export function pageHtmlReads(): HtmlReads | undefined {
  return typeof window === 'undefined' ? undefined : (window as Window & { __orchStartupReads?: HtmlReads }).__orchStartupReads;
}
