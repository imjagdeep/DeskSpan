// Share screen: start sharing, the QR code and code for a new device,
// devices asking to connect, and the screens connected now.

import { useEffect, useState } from "react";
import { PageHeader } from "../components/Common";
import { ShareIcon } from "../components/Icons";
import { api, type RequestView, type VideoStatus } from "../lib/api";
import { mmss, refreshHub, since, useHub } from "../lib/hub";
import { toast, toastError, useAppInfo } from "../lib/store";

export function Share() {
  const hub = useHub();
  const info = useAppInfo();
  const [busy, setBusy] = useState(false);
  const [video, setVideo] = useState<VideoStatus | null>(null);
  const watching = (hub?.sessions.length ?? 0) > 0;

  // The picture's state, every second while someone is connected.
  useEffect(() => {
    if (!watching) {
      setVideo(null);
      return;
    }
    const read = () => api.videoStatus().then(setVideo).catch(() => undefined);
    void read();
    const t = window.setInterval(read, 1000);
    return () => window.clearInterval(t);
  }, [watching]);

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
        <section className="card span-hero" aria-label="Sharing">
          <div className="hero-main">
            <div className="hero-state">
              <span className="hero-dot off" />
              Not sharing
            </div>
            <div className="hero-big">Use a phone, tablet or another computer as a screen</div>
            <p className="hero-text">
              Start sharing, then scan the QR code with the other device (or open the link and type the code). It needs nothing installed: just
              a browser on the same Wi-Fi or network.
            </p>
          </div>
          <button className="hero-btn" disabled={busy} onClick={() => void run(api.startSharing)}>
            <ShareIcon size={18} />
            Start sharing
          </button>
        </section>
        <Help />
      </div>
    );
  }

  const n = hub.sessions.length;
  return (
    <div className="page">
      <PageHeader title="Share screen" />

      <section className="card span-hero" aria-label="Sharing">
        <div className="hero-main">
          <div className="hero-state">
            <span className="hero-dot" />
            Sharing{info ? ` ${info.host_name}` : ""}
          </div>
          <div className="hero-big">{n === 0 ? "Waiting for a screen" : n === 1 ? "1 screen connected" : `${n} screens connected`}</div>
          <div className="hero-pills">
            {video && <span className="hero-pill">{pictureText(video)}</span>}
            {hub.requests.length > 0 && (
              <span className="hero-pill">{hub.requests.length === 1 ? "1 device asking to connect" : `${hub.requests.length} devices asking to connect`}</span>
            )}
            {hub.usb.setup === "ready" && <span className="hero-pill">USB cable on</span>}
          </div>
        </div>
        <button className="hero-btn" disabled={busy} onClick={() => void run(api.stopSharing)}>
          Stop sharing
        </button>
      </section>

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
          {video?.state === "error" && <span className="error-text small">No picture: {video.error}</span>}
        </div>
        {hub.sessions.length === 0 ? (
          <div className="muted small">None yet. Devices you allowed before reconnect by themselves when their page is open.</div>
        ) : (
          hub.sessions.map((s) => (
            <div key={s.session} className="list-row">
              <div>
                <div>
                  {s.name}
                  <span className={s.screen_on ? "badge" : "badge muted-badge"}>{s.screen_on ? "Screen" : "Connected"}</span>
                  {s.via_usb && <span className="badge muted-badge">USB cable</span>}
                </div>
                <div className="device-meta">
                  {s.address}
                  {s.screen ? ` · ${Math.round(s.screen.w * s.screen.dpr)}×${Math.round(s.screen.h * s.screen.dpr)}` : ""} · since {since(s.since)}
                </div>
              </div>
            </div>
          ))
        )}
      </div>
      <ExtendDriverCard />
      <UsbCard />
      <Help />
    </div>
  );
}

/** Windows: extending needs the virtual display driver, installed once. */
function ExtendDriverCard() {
  const [status, setStatus] = useState<"installed" | "missing" | "not-needed" | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.extendDriverStatus().then(setStatus).catch(() => setStatus(null));
  }, []);
  if (status !== "missing") return null;
  const install = async () => {
    setBusy(true);
    try {
      await api.installExtendDriver();
      setStatus(await api.extendDriverStatus());
      toast("Display driver installed: devices set to Extend get their own monitor");
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="card">
      <h2>Extend on Windows</h2>
      <p className="muted small">
        To use a device as an extra monitor (not just a copy), Windows needs a display driver: the open-source Virtual Display Driver, signed
        by the SignPath Foundation. DeskSpan downloads it from GitHub, checks it, and installs it. Windows asks for administrator permission
        once.
      </p>
      <button className="primary" disabled={busy} onClick={() => void install()}>
        {busy ? "Installing… (accept the Windows prompt)" : "Install display driver"}
      </button>
    </div>
  );
}

/** Android phones by USB cable (adb). */
function UsbCard() {
  const hub = useHub();
  const [busy, setBusy] = useState(false);
  if (!hub) return null;
  const u = hub.usb;
  const setUp = async () => {
    setBusy(true);
    try {
      await api.usbSetup();
      await refreshHub();
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="card">
      <div className="card-head">
        <h2>Android by USB cable</h2>
        {u.setup === "ready" && (
          <button className="link" onClick={() => void api.usbOff().then(refreshHub).catch(toastError)}>
            Turn off
          </button>
        )}
      </div>
      {u.setup !== "ready" ? (
        <>
          <p className="muted small">
            Faster and steadier than Wi-Fi. Uses Google's Android tools (adb); if they aren't on this computer, DeskSpan downloads them
            once from Google (about 10–16 MB).
          </p>
          {u.error && <p className="error-text small">{u.error}</p>}
          <button className="primary" disabled={busy || u.setup === "preparing"} onClick={() => void setUp()}>
            {u.setup === "preparing" || busy ? "Setting up…" : "Set up USB cable"}
          </button>
        </>
      ) : u.phones.length === 0 ? (
        <p className="muted small">
          Plug the phone in. Once only, on the phone: Settings › About phone › Software information › tap <b>Build number</b> 7 times, then
          Settings › Developer options › turn on <b>USB debugging</b>. DeskSpan then opens itself on the phone.
        </p>
      ) : (
        u.phones.map((p) => (
          <div key={p.serial} className="list-row">
            <div>
              <div>{p.model}</div>
              <div className="device-meta">
                {p.state === "ready"
                  ? "Connected by cable · DeskSpan opens on the phone"
                  : p.state === "allow"
                    ? "Tap Allow on the phone (USB debugging)"
                    : "Not responding: unplug and plug in again"}
              </div>
            </div>
          </div>
        ))
      )}
    </div>
  );
}

/** What the picture is doing: getting the encoder, streaming, or why not. */
function pictureText(v: VideoStatus): string {
  if (v.state === "preparing") return "Getting the video encoder from Cisco (once, about 1 MB)…";
  if (v.state === "error") return "No picture";
  if (v.state === "streaming" && v.width > 0) return `Main screen · ${v.width}×${v.height} · ${v.fps} fps`;
  return "Starting the picture…";
}

/** "Allow <device>?" */
function Ask({ r }: { r: RequestView }) {
  // A computer connecting from DeskSpan wants its mouse and keyboard to work.
  const [control, setControl] = useState(r.computer);
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
      <div className="muted small">
        {r.computer ? "Another computer with DeskSpan. " : ""}Only allow devices you know. It will see this computer's screen while you
        share.
      </div>
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
