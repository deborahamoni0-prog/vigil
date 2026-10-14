// Everything that talks to the chain directly: the RPC connection, contract ABIs and transactions.
import { ethers } from "ethers";

// Set VITE_RPC_URL at build time to point the app at another chain. Localhost is the default.
export const RPC_URL = import.meta.env.VITE_RPC_URL ?? "http://127.0.0.1:8545";
export const provider = new ethers.JsonRpcProvider(RPC_URL, undefined, { staticNetwork: true });
export const MAX_UINT = ethers.MaxUint256;

// The functions this page calls on the pool and the tokens.
export const POOL_ABI = [
  "function depositCollateral(address asset, uint256 amount)",
  "function withdrawCollateral(address asset, uint256 amount)",
  "function borrow(uint256 amount)",
  "function repay(uint256 amount)",
  "function nonceUsed(address user, uint256 nonce) view returns (bool)",
];
export const TOKEN_ABI = [
  "function balanceOf(address) view returns (uint256)",
  "function allowance(address owner, address spender) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
  "function mint(address to, uint256 amount)",
];

// The shape of a liquidation intent. The signature covers exactly these fields.
export const INTENT_TYPES = {
  LiquidationIntent: [
    { name: "borrower", type: "address" },
    { name: "collateralAsset", type: "address" },
    { name: "maxRepay", type: "uint256" },
    { name: "minCollateralOut", type: "uint256" },
    { name: "nonce", type: "uint256" },
    { name: "deadline", type: "uint256" },
  ],
};

// Sends a transaction from `from` and waits until it is mined.
// The gas limit is the node's estimate plus 30%: the estimate can come out too low when interest
// accrues between estimating and mining, which made some repayments run out of gas.
export async function sendTransaction(
  from: string,
  contract: ethers.Contract,
  method: string,
  args: unknown[],
): Promise<ethers.ContractTransactionReceipt | null> {
  const signer = await provider.getSigner(from);
  const connected = contract.connect(signer) as ethers.Contract;
  const gasLimit = ((await connected.getFunction(method).estimateGas(...args)) * 13n) / 10n;
  const tx = await connected.getFunction(method)(...args, { gasLimit });
  return tx.wait();
}

// Approves a token for the spender, but only when the current allowance is too small.
export async function approveIfNeeded(
  from: string,
  token: ethers.Contract,
  spender: string,
  amount: bigint,
): Promise<void> {
  const current: bigint = await token.allowance(from, spender);
  if (current < amount) {
    await sendTransaction(from, token, "approve", [spender, amount]);
  }
}

// Turns typed text into whole units. Throws a clear message when the text is not a valid amount.
export function parseAmount(text: string, decimals: number, allowZero = false): bigint {
  const typed = text.trim();
  if (typed === "") throw new Error("enter an amount");
  if (!/^\d*\.?\d*$/.test(typed) || typed === ".") throw new Error("the amount must be a number");
  const decimalPart = typed.split(".")[1] ?? "";
  if (decimalPart.length > decimals) throw new Error(`at most ${decimals} decimal places`);
  const units = ethers.parseUnits(typed, decimals);
  if (units === 0n && !allowZero) throw new Error("the amount must be above zero");
  return units;
}

// A short, readable reason for a failed transaction.
export function reasonOf(err: unknown): string {
  const e = err as { reason?: string; shortMessage?: string; message?: string; info?: { error?: { message?: string } } };
  const reason = e.reason || e.shortMessage || e.info?.error?.message || e.message || "something went wrong";
  // A revert usually means the price or the interest moved after we read the numbers.
  if (reason.includes("reverted")) {
    return "the contract refused it (the price or interest may have moved). Try a slightly smaller amount";
  }
  return reason;
}
