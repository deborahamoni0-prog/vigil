// Calls to the Rust gateway (services/gateway). The gateway serves this page too.
import type { AccountView, ChainSnapshot, Config, IntentRow } from "./types.ts";

async function getJson<T>(path: string): Promise<T> {
  const response = await fetch(path);
  if (!response.ok) throw new Error((await response.text()) || `${path} returned ${response.status}`);
  return (await response.json()) as T;
}

export const fetchConfig = () => getJson<Config>("/api/config");
export const fetchProtocol = () => getJson<ChainSnapshot>("/api/protocol");
export const fetchAccount = (who: string) => getJson<AccountView>(`/api/account/${who}`);
export const fetchIntents = () => getJson<IntentRow[]>("/api/intents");

export async function submitIntent(body: Record<string, string>): Promise<{ accepted: boolean; error?: string }> {
  const response = await fetch("/api/intents", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const result = (await response.json()) as { accepted: boolean; error?: string };
  if (!response.ok || !result.accepted) throw new Error(result.error ?? "the gateway rejected the intent");
  return result;
}
