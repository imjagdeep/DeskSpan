// The sharing status, kept fresh: the Rust side says "hub-changed" whenever
// a device connects, asks or leaves; the code's countdown ticks here.

import { listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";
import { api, type HubStatus } from "./api";
import { createStore, toastError } from "./store";

const hubStore = createStore<HubStatus | null>(null);
export const useHub = () => useSyncExternalStore(hubStore.subscribe, hubStore.get);

export async function refreshHub() {
  try {
    hubStore.set(await api.hubStatus());
  } catch (e) {
    toastError(e);
  }
}

/** Start listening for changes; returns the stop function. */
export function watchHub(): () => void {
  void refreshHub();
  const un = listen("hub-changed", () => void refreshHub());
  // The link and code expire; asking again makes new ones.
  const tick = window.setInterval(() => {
    const s = hubStore.get();
    if (s?.running && s.expires_in <= 1) void refreshHub();
    else if (s?.running) hubStore.set({ ...s, expires_in: Math.max(0, s.expires_in - 1) });
  }, 1000);
  return () => {
    window.clearInterval(tick);
    void un.then((f) => f());
  };
}

/** "9:41" left on the code */
export function mmss(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** "3 minutes ago", "yesterday", or a date */
export function since(unixSeconds: number, now = Date.now()): string {
  const s = Math.max(0, Math.round(now / 1000 - unixSeconds));
  if (s < 90) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} minutes ago`;
  const h = Math.round(m / 60);
  if (h < 24) return h === 1 ? "an hour ago" : `${h} hours ago`;
  const d = Math.round(h / 24);
  if (d === 1) return "yesterday";
  if (d < 14) return `${d} days ago`;
  return new Date(unixSeconds * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}
