/** An observed authority change retires pending private reads, including acknowledgement reads. */
let current=Symbol();
export const poolReadCut=():symbol=>current;
export const samePoolReadCut=(before:symbol):boolean=>before===current;
export function retirePoolReads():void { current=Symbol(); }
