/** CSS minification may rewrite milliseconds as seconds; JS clocks use ms. */
export function cssTimeMs(value:string,fallback:number):number {
  const match=value.trim().match(/^(\d*\.?\d+)\s*(ms|s)$/i);
  if(!match)return fallback;
  const amount=Number(match[1]);
  if(!Number.isFinite(amount))return fallback;
  return match[2].toLowerCase()==='s'?amount*1000:amount;
}
