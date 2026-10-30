/** The look of a link in running text and beside a form: the accent's colour, and a line under it when it is pointed at. */
export const linkClasses = 'rounded-sm font-medium text-accent-text underline-offset-2 hover:underline';

/** The same for a button that reads as a link: an act that changes what is on the page and leads nowhere. */
export const linkButtonClasses = `${linkClasses} cursor-pointer disabled:cursor-not-allowed disabled:text-text-disabled disabled:no-underline`;

/** A button in a line of quiet text that keeps the text's colour and is underlined when it is pointed at: a time that opens the history, who may read it that opens Share. */
export const quietButtonClasses = 'rounded-sm underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-focus';
