// Devices the user has allowed: control on/off, rename, remove.

import { useCallback, useEffect, useState } from "react";
import { Empty, PageHeader } from "../components/Common";
import { DevicesIcon } from "../components/Icons";
import { api, type Device } from "../lib/api";
import { since, useHub } from "../lib/hub";
import { toast, toastError } from "../lib/store";

export function Devices() {
  const hub = useHub();
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
