/** The inert converter's ordinary block pieces; identifiers are assigned by the creation producer. */
import type { BlockContent, BlockProps, BlockType } from '../frontend/src/generated/api-types';

export interface HtmlPiece { type: BlockType; props: BlockProps; content: BlockContent; children: HtmlPiece[] }
/** A picture the page names: the image block that shows it holds `id` as its file until the picture is settled. */
export interface HtmlImage { id: string; src: string; alt: string }
export interface ConvertedHtml { pieces: HtmlPiece[]; warnings: string[]; images: HtmlImage[] }
export function readHtml(source: string): ConvertedHtml;
