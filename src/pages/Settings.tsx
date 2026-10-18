import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { useEffect, useState } from "react";
import { PageHeader } from "../components/Common";
import { DeskLabTools } from "../components/DeskLab";
import { api, type Settings } from "../lib/api";
import { setAutoUpdate, setTheme, toastError, useAppInfo, useAutoUpdate, useTheme, type Theme } from "../lib/store";
import { checkAndDownload, installNow, useUpdate } from "../lib/updates";

export function SettingsPage() {
  const info = useAppInfo();
  const theme = useTheme();
  const auto = useAutoUpdate();
  const st = useUpdate();
  const busy = st.kind === "checking" || st.kind === "downloading" || st.kind === "installing";
  const [s, setS] = useState<Settings | null>(null);
  const [port, setPort] = useState("");
  const [startup, setStartup] = useState<boolean | null>(null);

  useEffect(() => {
    api
      .getSettings()
      .then((v) => {
        setS(v);
        setPort(String(v.port));
      })
      .catch(toastError);
    isEnabled()
      .then(setStartup)
      .catch(() => setStartup(null));
  }, []);

  const save = async (patch: Partial<Settings>) => {
    if (!s) return;
    try {
      const next = await api.saveSettings({ ...s, ...patch });
      setS(next);
      setPort(String(next.port));
    } catch (e) {
      toastError(e);
    }
  };

  const savePort = () => {
    const n = Number(port);
    if (!Number.isInteger(n) || n < 1024 || n > 65535) {
      toastError("Choose a port between 1024 and 65535.");
      if (s) setPort(String(s.port));
      return;
    }
    if (s && n !== s.port) void save({ port: n });
  };

  const setStartWith = async (on: boolean) => {
    try {
      await (on ? enable() : disable());
      setStartup(await isEnabled());
    } catch (e) {
      toastError(e);
    }
  };

  const windows = info?.platform === "windows";

  return (
    <div className="page">
      <PageHeader title="Settings" />

      {s && (
        <div className="card">
          <h2>Network</h2>
          <label className="check-row">
            <input type="checkbox" checked={s.listen_everywhere} onChange={(e) => void save({ listen_everywhere: e.target.checked })} />
            <span>
              Also allow connections over VPNs and other networks
              <span className="muted small block">
                Off: only devices on your home or office network (and USB tethering) can reach DeskSpan. Turn on to use it over Tailscale or a
                similar VPN.
              </span>
            </span>
          </label>
          <div className="setting-line">
            <span>
              Port
              <span className="muted small block">Change it only if another app already uses {s.port}.</span>
            </span>
            <input
              className="mono"
              style={{ width: 90 }}
              inputMode="numeric"
              aria-label="Port"
              value={port}
              onChange={(e) => setPort(e.target.value.replace(/\D/g, "").slice(0, 5))}
              onBlur={savePort}
              onKeyDown={(e) => e.key === "Enter" && savePort()}
            />
          </div>
        </div>
      )}

      {s && (
        <div className="card">
          <h2>Start-up</h2>
          <label className="check-row">
            <input type="checkbox" checked={s.start_on_launch} onChange={(e) => void save({ start_on_launch: e.target.checked })} />
            <span>
              Start sharing when DeskSpan starts
              <span className="muted small block">Devices you've allowed can then connect without you opening DeskSpan first.</span>
            </span>
          </label>
          {startup !== null && (
            <label className="check-row">
              <input type="checkbox" checked={startup} onChange={(e) => void setStartWith(e.target.checked)} />
              <span>
                Start with the computer
                <span className="muted small block">DeskSpan waits quietly in the {windows ? "tray" : "menu bar"}.</span>
              </span>
            </label>
          )}
        </div>
      )}

      <div className="card">
        <h2>Appearance</h2>
        <div className="segmented" role="group" aria-label="Theme">
          {(["system", "light", "dark"] as Theme[]).map((t) => (
            <button key={t} className={theme === t ? "active" : ""} onClick={() => setTheme(t)}>
              {t === "system" ? "System" : t === "light" ? "Light" : "Dark"}
            </button>
          ))}
        </div>
      </div>

      <div className="card">
        <div className="about-row">
          <div>
            <div className="about-title">
              DeskSpan <span className="muted">{info?.version}</span>
            </div>
            <div className="muted small">
              {st.kind === "idle" && (auto ? "Updates download automatically." : "Automatic updates are off.")}
              {st.kind === "checking" && "Checking GitHub for a newer version…"}
              {st.kind === "latest" && "You have the latest version."}
              {st.kind === "downloading" && `Downloading ${st.version}${st.pct != null ? ` (${st.pct}%)` : ""}…`}
              {st.kind === "ready" && `Version ${st.version} is downloaded and ready.`}
              {st.kind === "installing" && `Installing ${st.version}… DeskSpan restarts by itself.`}
              {st.kind === "error" && <span className="error-text">Couldn't update: {st.message}</span>}
            </div>
          </div>
          {st.kind === "ready" ? (
            <button className="primary" onClick={() => void installNow()}>
              Restart to update
            </button>
          ) : (
            <button disabled={busy} onClick={() => void checkAndDownload(true)}>
              Check for updates
            </button>
          )}
        </div>
        <label className="check-row">
          <input type="checkbox" checked={auto} onChange={(e) => setAutoUpdate(e.target.checked)} />
          <span>
            Download updates automatically
            <span className="muted small block">
              Checks GitHub once a day, downloads new versions in the background (signature-checked) and asks you to restart. Your screen
              only ever goes to the devices you allow, over your own network.
            </span>
          </span>
        </label>
        <div className="more-tools">
          <DeskLabTools own="deskspan" />
          <div className="muted small made-by">
            OpenH264 Video Codec provided by Cisco Systems, Inc. (downloaded from Cisco the first time you share; BSD licence).
          </div>
          <div className="muted small made-by">
            Open source, MIT licence. Made by Jagdeep Sandhu ·{" "}
            <button className="link" onClick={() => api.openLink("developer").catch(toastError)}>
              github.com/imjagdeep
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
