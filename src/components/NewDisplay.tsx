// "Display connected": when a monitor is plugged in (HDMI, DisplayPort,
// USB-C, VGA…), ask whether to extend the desktop onto it or duplicate it.

import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { api, type Screen } from "../lib/api";
import { toast, toastError } from "../lib/store";

export function NewDisplay({ onArrange }: { onArrange: () => void }) {
  const [screen, setScreen] = useState<Screen | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const un = listen<{ screen: Screen }>("screen-added", (e) => {
      setScreen(e.payload.screen);
      // Bring the window up so the question is seen.
      const w = getCurrentWindow();
      void w
        .show()
        .then(() => w.setFocus())
        .catch(() => undefined);
    });
    return () => void un.then((f) => f());
  }, []);

  if (!screen) return null;

  const choose = async (duplicate: boolean) => {
    setBusy(true);
    try {
      await api.screensSetMode(duplicate);
      toast(duplicate ? `${screen.name} shows a copy of your screen` : `Your desktop now continues onto ${screen.name}`);
      setScreen(null);
    } catch (e) {
      toastError(e);
    }
    setBusy(false);
  };

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Display connected">
      <div className="modal new-display">
        <h2>Display connected</h2>
        <p>
          <strong>{screen.name}</strong>
          <span className="muted">
            {" "}
            · {screen.connection} · {screen.w}×{screen.h}
          </span>
        </p>
        <div className="choice-pair">
          <button className="choice" disabled={busy} onClick={() => void choose(false)}>
            <strong>Extend</strong>
            <span className="muted small">More room: the desktop continues onto it.</span>
          </button>
          <button className="choice" disabled={busy} onClick={() => void choose(true)}>
            <strong>Duplicate</strong>
            <span className="muted small">The same picture on both, e.g. for presenting.</span>
          </button>
        </div>
        <div className="modal-actions">
          <button
            onClick={() => {
              setScreen(null);
              onArrange();
            }}
          >
            Arrange…
          </button>
          <button onClick={() => setScreen(null)}>Not now</button>
        </div>
      </div>
    </div>
  );
}
