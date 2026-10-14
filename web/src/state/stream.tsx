// One live connection to the gateway (Server-Sent Events) shared by every component on the page.
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import type { ActivityMessage, ChainSnapshot } from "../lib/types.ts";

export interface DbSnapshot {
  indexerCursor: number | null;
  processedEvents: number;
  borrowingPositions: number;
  intents: number;
  liquidatorTxs: number;
}

export interface StreamState {
  connected: boolean;
  chain: ChainSnapshot | null;
  lastBlockAt: number | null;
  db: DbSnapshot | null;
  activity: ActivityMessage[];          // newest first, at most 200
  counts: { events: number; blocks: number; signals: number; retracts: number };
  priceHistory: Record<string, bigint[]>; // recent prices per collateral name, for the small charts
}

const initial: StreamState = {
  connected: false,
  chain: null,
  lastBlockAt: null,
  db: null,
  activity: [],
  counts: { events: 0, blocks: 0, signals: 0, retracts: 0 },
  priceHistory: {},
};

const StreamContext = createContext<StreamState>(initial);

export function StreamProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<StreamState>(initial);

  useEffect(() => {
    const source = new EventSource("/api/stream");
    source.onopen = () => setState((s) => ({ ...s, connected: true }));
    source.onerror = () => setState((s) => ({ ...s, connected: false }));
    source.onmessage = (event) => {
      let msg: { kind: string; data: unknown; stage?: string };
      try {
        msg = JSON.parse(event.data);
      } catch {
        return;
      }
      if (msg.kind === "chain") {
        const chain = msg.data as ChainSnapshot;
        setState((s) => {
          const priceHistory = { ...s.priceHistory };
          for (const p of chain.prices) {
            if (p.price_wad === "frozen") continue;
            const history = [...(priceHistory[p.name] ?? []), BigInt(p.price_wad)];
            priceHistory[p.name] = history.slice(-60);
          }
          return { ...s, chain, lastBlockAt: Date.now(), priceHistory };
        });
      } else if (msg.kind === "db") {
        setState((s) => ({ ...s, db: msg.data as DbSnapshot }));
      } else if (msg.kind === "nats") {
        const stage = msg.stage ?? "";
        const message: ActivityMessage = { stage, data: msg.data as ActivityMessage["data"] };
        setState((s) => {
          const counts = { ...s.counts };
          if (stage === "events") counts.events++;
          if (stage === "blocks") counts.blocks++;
          if (stage === "signals") counts.signals++;
          if (stage === "events" && message.data.kind === "retract") counts.retracts++;
          const keep = stage === "events" || stage === "signals";
          return {
            ...s,
            counts,
            activity: keep ? [message, ...s.activity].slice(0, 200) : s.activity,
          };
        });
      }
    };
    return () => source.close();
  }, []);

  return <StreamContext.Provider value={state}>{children}</StreamContext.Provider>;
}

export function useStream(): StreamState {
  return useContext(StreamContext);
}
