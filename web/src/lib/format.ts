// Turns whole-number amounts and prices into readable text.
import { ethers } from "ethers";

// 1234567n with 6 decimals reads "1.23". Commas and up to `places` decimals.
export function fmt(units: bigint, decimals: number, places = 2): string {
  const n = Number(ethers.formatUnits(units, decimals));
  // The minimum can never be above the maximum, or the browser throws a RangeError.
  return n.toLocaleString("en-US", { maximumFractionDigits: places, minimumFractionDigits: Math.min(2, places) });
}

// An 18-decimal USD value as "$1,234.56".
export function dollars(wad: bigint): string {
  return "$" + fmt(wad, 18, 2);
}

// 0x1234…abcd
export function short(address: string): string {
  return address.slice(0, 6) + "…" + address.slice(-4);
}
