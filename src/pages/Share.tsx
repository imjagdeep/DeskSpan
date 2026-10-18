// Share screen: start sharing, the QR code and code for a new device,
// devices asking to connect, and the screens connected now.

import { useState } from "react";
import { PageHeader } from "../components/Common";
import { ShareIcon } from "../components/Icons";
import { api, type RequestView } from "../lib/api";
import { mmss, refreshHub, since, useHub } from "../lib/hub";
import { toast, toastError, useAppInfo } from "../lib/store";

export function Share() {
  const hub = useHub();
  const info = useAppInfo();
  const [busy, setBusy] = useState(false);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
      await refreshHub();
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(false);
    }
  };

  if (!hub) return <div className="page muted">…</div>;

  if (!hub.running) {
    return (
      <div className="page">
        <PageHeader title="Share screen" />
        <div className="card share-off">
          <span className="dropzone-icon">
            <ShareIcon size={28} />
          </span>
          <div className="dropzone-title">Use a phone, tablet or another computer as a screen</div>
          <p className="muted small">
            Start sharing, then scan the QR code with the other device (or open the link and type the code). It needs nothing installed:
            just a browser on the same Wi-Fi or network.
          </p>
          <div className="row" style={{ justifyContent: "center" }}>
            <button className="primary" disabled={busy} onClick={() => void run(api.startSharing)}>
              Start sharing
            </button>
          </div>
        </div>
        <Help />
      </div>
    );
  }

  return (
    <div className="page">
      <PageHeader title="Share screen" subtitle={info ? `Sharing ${info.host_name}` : undefined}>
        <button disabled={busy} onClick={() => void run(api.stopSharing)}>
          Stop sharing
        </button>
      </PageHeader>

      {hub.requests.map((r) => (
        <Ask key={r.id} r={r} />
      ))}

      <div className="card">
        <h2>Connect a new device</h2>
        <div className="share-grid">
          <div className="qr">{hub.qr_svg ? <img alt="QR code with the link for a new device" src={`data:image/svg+xml;utf8,${encodeURIComponent(hub.qr_svg)}`} /> : null}</div>
          <div>
            <div className="muted small">Scan with the phone or tablet's camera, or open one of these in its browser:</div>
            <div className="url-list">
              {hub.urls.map((u) => (
                <span key={u} className="mono">
                  {u}
                </span>
              ))}
            </div>
            <div className="muted small">and type this code:</div>
            <div className="pair-code" aria-label={`Code ${hub.code ?? ""}`}>
              {hub.code ?? "……"}
            </div>
            <div className="muted small">
              Works once, for {mmss(hub.expires_in)} more ·{" "}
              <button className="link" disabled={busy} onClick={() => void run(api.newPairing)}>
                New code
              </button>
            </div>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <h2>Connected screens</h2>
        </div>
        {hub.sessions.length === 0 ? (
          <div className="muted small">None yet. Devices you allowed before reconnect by themselves when their page is open.</div>
        ) : (
          hub.sessions.map((s) => (
            <div key={s.session} className="list-row">
              <div>
                <div>{s.name}</div>
                <div className="device-meta">
                  {s.address}
                  {s.screen ? ` · ${Math.round(s.screen.w * s.screen.dpr)}×${Math.round(s.screen.h * s.screen.dpr)}` : ""} · since {since(s.since)}
                </div>
              </div>
              <div className="actions">
                <span className="badge warn" title="Showing the picture comes with the next update">
                  Picture coming soon
                </span>
              </div>
            </div>
          ))
        )}
      </div>
      <Help />
    </div>
  );
}

/** "Allow <device>?" */
function Ask({ r }: { r: RequestView }) {
  const [control, setControl] = useState(false);
  const answer = (allow: boolean) =>
    api
      .answerRequest(r.id, allow, control)
      .then(() => toast(allow ? `${r.name} can connect now` : `${r.name} was turned away`))
      .catch(toastError);
  return (
    <div className="card request" role="alertdialog" aria-label={`${r.name} wants to connect`}>
      <h2>
        {r.name} wants to connect <span className="muted small">{r.address}</span>
      </h2>
      <div className="muted small">Only allow devices you know. It will see this computer's screen while you share.</div>
      <label className="check-row">
        <input type="checkbox" checked={control} onChange={(e) => setControl(e.target.checked)} />
        <span>
          Also let it control this computer
          <span className="muted small block">Touch, pen and mouse on the device act on this computer. You can change this later under Devices.</span>
        </span>
      </label>
      <div className="row">
        <button className="primary" onClick={() => void answer(true)}>
          Allow
        </button>
        <button onClick={() => void answer(false)}>Don't allow</button>
      </div>
    </div>
  );
}

function Help() {
  return (
    <div className="card">
      <h2>Good to know</h2>
      <ul className="muted small" style={{ margin: 0, paddingLeft: 18 }}>
        <li>Both devices must be on the same network. Nothing goes through the internet.</li>
        <li>With a cable: on Android, turn on USB tethering; the phone and computer are then on the same network.</li>
        <li>On a phone or iPad, "Add to Home Screen" from the browser's share menu opens the screen full size.</li>
      </ul>
    </div>
  );
}
