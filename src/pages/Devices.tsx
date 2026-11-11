// Devices the user has allowed: control on/off, rename, remove.

import { useCallback, useEffect, useState } from "react";
import { Empty, PageHeader } from "../components/Common";
import { DevicesIcon } from "../components/Icons";
import { api, type Device } from "../lib/api";
import { since, useHub } from "../lib/hub";
import { PRESETS, describe, presetOf } from "../lib/picture";
import { toast, toastError, useAppInfo } from "../lib/store";

export function Devices() {
  const hub = useHub();
  const info = useAppInfo();
  // Making a device's display the main screen works on macOS for now.
  const canMain = info?.platform === "macos";
  const [list, setList] = useState<Device[] | null>(null);
  const load = useCallback(() => api.devices().then(setList).catch(toastError), []);
  // The hub changes when a device is added, connects or is removed.
  useEffect(() => void load(), [load, hub]);

  const online = new Set(hub?.sessions.map((s) => s.device_id) ?? []);

  const [editing, setEditing] = useState<{ id: string; name: string } | null>(null);
  const rename = async () => {
    if (!editing) return;
    try {
      await api.renameDevice(editing.id, editing.name);
      setEditing(null);
      await load();
    } catch (e) {
      toastError(e);
    }
  };

  const remove = async (d: Device) => {
    try {
      await api.removeDevice(d.id);
      toast(`${d.name} removed. It needs a new code to connect again.`);
      await load();
    } catch (e) {
      toastError(e);
    }
  };

  const setExtend = async (d: Device, extend: boolean) => {
    try {
      await api.setExtend(d.id, extend);
      await load();
      if (online.has(d.id)) toast(`${d.name} switches to ${extend ? "extend" : "mirror"} the next time it connects.`);
    } catch (e) {
      toastError(e);
    }
  };

  const setMain = async (d: Device, on: boolean) => {
    try {
      await api.setMainDisplay(d.id, on);
      await load();
      if (online.has(d.id)) toast(`${d.name} ${on ? "becomes" : "stops being"} the main display the next time it connects.`);
    } catch (e) {
      toastError(e);
    }
  };

  const setPicture = async (d: Device, id: string) => {
    const preset = PRESETS.find((p) => p.id === id);
    if (!preset) return;
    try {
      await api.setPicture(d.id, preset.picture);
      await load();
      if (online.has(d.id)) toast(`${d.name} gets the new picture the next time it connects.`);
    } catch (e) {
      toastError(e);
    }
  };

  const setControl = async (d: Device, on: boolean) => {
    try {
      await api.setControl(d.id, on);
      await load();
    } catch (e) {
      toastError(e);
    }
  };

  return (
    <div className="page">
      <PageHeader title="Devices" subtitle="Phones, tablets and computers you've allowed" />
      {list === null ? null : list.length === 0 ? (
        <Empty Icon={DevicesIcon} title="No devices yet" text="Devices you allow on the Share screen page show up here." />
      ) : (
        <div className="card">
          {list.map((d) => (
            <div key={d.id} className="list-row">
              <div>
                {editing?.id === d.id ? (
                  <form
                    onSubmit={(e) => {
                      e.preventDefault();
                      void rename();
                    }}
                  >
                    <input
                      autoFocus
                      aria-label="Name for this device"
                      maxLength={60}
                      value={editing.name}
                      onChange={(e) => setEditing({ id: d.id, name: e.target.value })}
                      onKeyDown={(e) => e.key === "Escape" && setEditing(null)}
                    />
                    <span className="muted small"> Enter to save, Esc to cancel</span>
                  </form>
                ) : (
                  <div>
                    {d.name}
                    {online.has(d.id) && <span className="badge">Connected</span>}
                  </div>
                )}
                <div className="device-meta">
                  Added {since(d.added)} · last seen {since(d.last_seen)}
                </div>
              </div>
              <div className="actions">
                <div className="segmented small" role="group" aria-label={`${d.name}: extend or mirror`}>
                  <button
                    className={d.extend ? "active" : ""}
                    title="Its own extra display: drag windows onto it"
                    onClick={() => void setExtend(d, true)}
                  >
                    Extend
                  </button>
                  <button
                    className={d.extend ? "" : "active"}
                    title="Shows the same picture as the main screen"
                    onClick={() => void setExtend(d, false)}
                  >
                    Mirror
                  </button>
                </div>
                {canMain && d.extend && (
                  <label className="toggle" title="Its screen becomes the main display: the menu bar and new windows go there. For a Mac with no monitor, like a Mac mini.">
                    <input type="checkbox" checked={d.main} onChange={(e) => void setMain(d, e.target.checked)} />
                    <span>Main display</span>
                  </label>
                )}
                <select
                  aria-label={`${d.name}: picture`}
                  title={d.picture ? describe(d.picture) : "Follows the picture setting in Settings"}
                  value={presetOf(d.picture)}
                  onChange={(e) => void setPicture(d, e.target.value)}
                >
                  {presetOf(d.picture) === "custom" && d.picture && <option value="custom">{describe(d.picture)}</option>}
                  {PRESETS.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.label}
                    </option>
                  ))}
                </select>
                <label className="toggle" title="Touch, pen and mouse on the device act on this computer">
                  <input type="checkbox" checked={d.control} onChange={(e) => void setControl(d, e.target.checked)} />
                  <span>Can control</span>
                </label>
                <button className="link" onClick={() => setEditing({ id: d.id, name: d.name })}>
                  Rename
                </button>
                <button className="link danger" onClick={() => void remove(d)}>
                  Remove
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
