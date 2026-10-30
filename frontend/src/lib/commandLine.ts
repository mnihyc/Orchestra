/** Turn a displayed executable/argument command into argv; no expansion, substitution or shell operators. */
export function commandArguments(text:string):string[]|null {
  const args:string[]=[];
  let word='',quote:''|'"'|"'"='',started=false;
  for(let index=0;index<text.length;index+=1) {
    const char=text[index] ?? '';
    if(char==='\0') return null;
    if(quote!=="'" && char==='\\') {
      const next=text[index+1];
      if(next!==undefined && (next==='\\'||next==='"'||(quote==='' && (next==="'"||/\s/u.test(next))))) {
        word+=next;index+=1;started=true;continue;
      }
    }
    if(char==='"'||char==="'") {
      if(quote==='') {quote=char;started=true;continue;}
      if(quote===char) {quote='';continue;}
    }
    if(quote==='' && /\s/u.test(char)) {
      if(started) {args.push(word);word='';started=false;}
    } else {word+=char;started=true;}
  }
  if(quote!=='') return null;
  if(started) args.push(word);
  return args;
}

/** A readable command round-trips through the same argv parser, including quoted Windows paths. */
export function commandLine(args:readonly string[]):string {
  return args.map((word)=>/^[\w/.:@=+,-]+$/u.test(word)?word:'"'+word.replace(/\\/gu,'\\\\').replace(/"/gu,'\\"')+'"').join(' ');
}
