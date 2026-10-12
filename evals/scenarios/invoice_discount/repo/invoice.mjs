import {taxAmount} from "./tax.mjs";export function total(items,region,discount=0){const subtotal=items.reduce((a,b)=>a+b,0);return Math.round((subtotal+taxAmount(subtotal,region)-discount)*100)/100}
