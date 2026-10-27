// Every screen on the computer, like the system's display settings: drag to
// arrange, pick the main one, extend or duplicate. Real monitors (HDMI,
// DisplayPort, USB-C, VGA…) and DeskSpan's phone and tablet screens alike.

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { PageHeader } from "../components/Common";
import { api, type Screen, type ScreenLayout } from "../lib/api";
import { toast, toastError } from "../lib/store";

/** Snap distance, in desktop pixels. */
const SNAP = 120;

interface Pos {
  x: number;
  y: number;
}

/**
 * Put a dragged screen flush against its nearest neighbour (left/right/top/
 * bottom edge) so screens touch, as both systems need.
 */
export function snap(moving: Screen, at: Pos, others: Screen[]): Pos {
  let best: { pos: Pos; d: number } | null = null;
  for (const o of others) {
    const candidates: Pos[] = [
      { x: o.x - moving.w, y: clampY(at.y, o, moving) }, // left of it
      { x: o.x + o.w, y: clampY(at.y, o, moving) }, // right of it
      { x: clampX(at.x, o, moving), y: o.y - moving.h }, // above
      { x: clampX(at.x, o, moving), y: o.y + o.h }, // below
    ];
    for (const c of candidates) {
      const d = Math.hypot(c.x - at.x, c.y - at.y);
      if (!best || d < best.d) best = { pos: c, d };
    }
  }
  if (!best) return at;
  // Line up with an edge when close.
  const p = { ...best.pos };
  for (const o of others) {
    if (Math.abs(p.y - o.y) < SNAP) p.y = o.y;
    if (Math.abs(p.x - o.x) < SNAP) p.x = o.x;
  }
  return p;
}

// Keep the moving screen alongside `o` (sharing some edge length).
const clampY = (y: number, o: Screen, m: Screen) => Math.min(Math.max(y, o.y - m.h + 1), o.y + o.h - 1);
const clampX = (x: number, o: Screen, m: Screen) => Math.min(Math.max(x, o.x - m.w + 1), o.x + o.w - 1);

export function Displays() {
  const [layout, setLayout] = useState<ScreenLayout | null>(null);
  const [moved, setMoved] = useState<Record<string, Pos>>({});
  const [chosen, setChosen] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<{ id: string; start: Pos; from: Pos; scale: number } | null>(null);

  const load = useCallback(async () => {
    try {
      const l = await api.screensLayout();
      setLayout(l);
      setMoved({});
      setChosen((c) => c ?? l.screens.find((s) => s.main)?.id ?? null);
    } catch (e) {
      toastError(e);
    }
  }, []);
  useEffect(() => {
    void load();
    const un = listen("screens-changed", () => void load());
    return () => void un.then((f) => f());
  }, [load]);

  if (!layout) return <div className="page" />;

  // The extended screens (copies sit on top of their original).
  const screens = layout.screens.map((s) => ({ ...s, ...(moved[s.id] ?? {}) }));
  const shown = screens.filter((s) => !s.mirror_of);
  const minX = Math.min(...shown.map((s) => s.x));
  const minY = Math.min(...shown.map((s) => s.y));
  const maxX = Math.max(...shown.map((s) => s.x + s.w));
  const maxY = Math.max(...shown.map((s) => s.y + s.h));
  const width = box.current?.clientWidth ?? 640;
  const scale = Math.min((width - 40) / Math.max(1, maxX - minX), 260 / Math.max(1, maxY - minY));
  const dirty = Object.keys(moved).length > 0;
  const sel = screens.find((s) => s.id === chosen) ?? null;

  const onDown = (e: React.PointerEvent, s: Screen) => {
    setChosen(s.id);
    if (!layout.can_arrange || s.mirror_of || shown.length < 2) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag.current = { id: s.id, start: { x: e.clientX, y: e.clientY }, from: { x: s.x, y: s.y }, scale };
  };
  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const x = Math.round(d.from.x + (e.clientX - d.start.x) / d.scale);
    const y = Math.round(d.from.y + (e.clientY - d.start.y) / d.scale);
    setMoved((m) => ({ ...m, [d.id]: { x, y } }));
  };
  const onUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    const me = screens.find((s) => s.id === d.id);
    if (!me) return;
    const others = shown.filter((s) => s.id !== d.id);
    const p = snap(me, { x: me.x, y: me.y }, others);
    if (p.x === d.from.x && p.y === d.from.y) {
      setMoved((m) => {
        const { [d.id]: _gone, ...rest } = m;
        return rest;
      });
    } else setMoved((m) => ({ ...m, [d.id]: p }));
  };

  async function run(what: () => Promise<void>, done: string) {
    setBusy(true);
    try {
      await what();
      toast(done);
    } catch (e) {
      toastError(e);
    }
    setBusy(false);
    await load();
  }

  return (
    <div className="page">
      <PageHeader title="Displays" subtitle="Every screen on this computer: monitors on any cable, and DeskSpan's phone and tablet screens." />

      <div className="card">
        <div className="displays-head">
          <h2>Arrangement</h2>
          {layout.can_mirror && shown.length + screens.filter((s) => s.mirror_of).length > 1 && (
            <div className="segmented" role="group" aria-label="Extend or duplicate">
              <button
                className={layout.mode === "extend" ? "active" : ""}
                disabled={busy}
                onClick={() => layout.mode !== "extend" && void run(() => api.screensSetMode(false), "Screens extended")}
              >
                Extend
              </button>
              <button
                className={layout.mode === "duplicate" ? "active" : ""}
                disabled={busy}
                onClick={() => layout.mode !== "duplicate" && void run(() => api.screensSetMode(true), "Screens duplicated")}
              >
                Duplicate
              </button>
            </div>
          )}
        </div>
        <div className="display-map" ref={box} style={{ height: Math.max(140, (maxY - minY) * scale + 40) }} onPointerMove={onMove} onPointerUp={onUp}>
          {shown.map((s) => {
            const copies = screens.filter((c) => c.mirror_of === s.id);
            return (
              <button
                key={s.id}
                className={`display-tile${s.id === chosen ? " chosen" : ""}${s.deskspan ? " deskspan" : ""}`}
                style={{ left: 20 + (s.x - minX) * scale, top: 20 + (s.y - minY) * scale, width: s.w * scale - 4, height: s.h * scale - 4 }}
                onPointerDown={(e) => onDown(e, s)}
                title={`${s.name} · ${s.connection} · ${s.w}×${s.h}`}
              >
                <span className="display-num">{shown.indexOf(s) + 1}</span>
                <span className="display-name">{s.name}</span>
                <span className="display-meta">
                  {s.connection}
                  {s.main ? " · Main" : ""}
                  {copies.length ? ` · + ${copies.length} copy` : ""}
                </span>
              </button>
            );
          })}
        </div>
        {layout.can_arrange ? (
          <div className="display-actions">
            <span className="muted small">{shown.length > 1 ? "Drag the screens to match where they sit on your desk." : "Connect another screen to arrange them."}</span>
            <button disabled={!dirty || busy} onClick={() => setMoved({})}>
              Reset
            </button>
            <button
              className="primary"
              disabled={!dirty || busy}
              onClick={() => void run(() => api.screensArrange(shown.map((s) => ({ id: s.id, x: s.x, y: s.y }))), "Arrangement applied")}
            >
              Apply
            </button>
          </div>
        ) : (
          <p className="muted small">Arranging isn't available on Linux yet: use your desktop's display settings.</p>
        )}
      </div>

      {sel && (
        <div className="card">
          <h2>{sel.name}</h2>
          <div className="setting-line">
            <span>Connection</span>
            <span className="muted">{sel.connection}</span>
          </div>
          <div className="setting-line">
            <span>Resolution</span>
            <span className="muted mono">
              {sel.w} × {sel.h}
            </span>
          </div>
          <div className="setting-line">
            <span>
              Main display
              <span className="muted small block">Where the taskbar or menu bar and new windows go.</span>
            </span>
            {sel.main ? (
              <span className="muted">This is the main display</span>
            ) : (
              <button disabled={busy || !layout.can_arrange || !!sel.mirror_of} onClick={() => void run(() => api.screensSetMain(sel.id), `${sel.name} is now the main display`)}>
                Make main display
              </button>
            )}
          </div>
          {sel.deskspan && <p className="muted small">A DeskSpan screen: it goes away when the device disconnects.</p>}
        </div>
      )}
    </div>
  );
}
