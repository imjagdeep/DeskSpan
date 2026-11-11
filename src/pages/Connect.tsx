// Connect: show another computer's screen in a DeskSpan window. Computers
// that are sharing announce themselves on the network; one can also be
// opened by its address.

import { useEffect, useState } from "react";
import { PageHeader } from "../components/Common";
import { ConnectIcon } from "../components/Icons";
import { api, type FoundComputer } from "../lib/api";
import { toastError } from "../lib/store";

const RECENT = "deskspan-recent-computers";

interface Recent {
  name: string;
  address: string;
}

function loadRecent(): Recent[] {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(RECENT) ?? "[]");
    if (!Array.isArray(v)) return [];
    return v.filter((r): r is Recent => typeof r === "object" && r !== null && typeof r.name === "string" && typeof r.address === "string");
  } catch {
    return [];
  }
}

function remember(r: Recent) {
  try {
    const next = [r, ...loadRecent().filter((x) => x.address !== r.address)].slice(0, 5);
    localStorage.setItem(RECENT, JSON.stringify(next));
  } catch {
    // Not remembered: it still shows while it is sharing.
  }
}

export function Connect() {
  const [found, setFound] = useState<FoundComputer[] | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [recent, setRecent] = useState<Recent[]>(loadRecent);
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState(false);

  // Look again every few seconds while the page is open.
  useEffect(() => {
    let alive = true;
    let timer = 0;
    const look = () => {
      api
        .findComputers()
        .then((list) => {
          if (!alive) return;
          setFound(list);
          setSearchError(null);
        })
        .catch((e: unknown) => {
          if (alive) setSearchError(String(e));
        })
        .finally(() => {
          if (alive) timer = window.setTimeout(look, 3000);
        });
    };
    look();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, []);

  const open = async (name: string, addr: string) => {
    setBusy(true);
    try {
      await api.openViewer(name, addr);
      remember({ name, address: addr });
      setRecent(loadRecent());
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(false);
    }
  };

  const foundAddresses = new Set((found ?? []).map((c) => c.address));
  const olderOnes = recent.filter((r) => !foundAddresses.has(r.address));

  return (
    <div className="page">
      <PageHeader title="Connect" subtitle="See and control another computer's screen in a DeskSpan window" />

      <div className="card">
        <div className="card-head">
          <h2>Computers on your network</h2>
          <span className="muted small">{found === null ? "Looking…" : "Updates by itself"}</span>
        </div>
        {searchError && <p className="error-text small">{searchError}</p>}
        {found !== null && found.length === 0 ? (
          <div className="connect-empty">
            <ConnectIcon size={28} />
            <p className="muted small">
              No other computer is sharing right now. On the other computer, open DeskSpan and press <b>Start sharing</b>: it shows up
              here within a few seconds.
            </p>
          </div>
        ) : (
          (found ?? []).map((c) => (
            <div key={c.address} className="list-row">
              <div>
                <div>{c.name}</div>
                <div className="device-meta mono">{c.address}</div>
              </div>
              <div className="actions">
                <button className="primary" disabled={busy} onClick={() => void open(c.name, c.address)}>
                  Show screen
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      {olderOnes.length > 0 && (
        <div className="card">
          <h2>Recent</h2>
          {olderOnes.map((r) => (
            <div key={r.address} className="list-row">
              <div>
                <div>{r.name}</div>
                <div className="device-meta mono">{r.address} · not sharing right now, or not found</div>
              </div>
              <div className="actions">
                <button disabled={busy} onClick={() => void open(r.name, r.address)}>
                  Try anyway
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="card">
        <h2>Connect by address</h2>
        <p className="muted small">If a computer doesn't show up, type the address its DeskSpan shows under Share screen.</p>
        <form
          className="row connect-form"
          onSubmit={(e) => {
            e.preventDefault();
            const a = address.trim();
            if (a) void open(a, a);
          }}
        >
          <input
            aria-label="The other computer's address"
            placeholder="192.168.1.20 or 192.168.1.20:47800"
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            spellCheck={false}
            autoCapitalize="off"
          />
          <button type="submit" className="primary" disabled={busy || !address.trim()}>
            Connect
          </button>
        </form>
      </div>

      <div className="card">
        <h2>Good to know</h2>
        <ul className="muted small" style={{ margin: 0, paddingLeft: 18 }}>
          <li>The first time, type the code the other computer shows, and allow this computer there.</li>
          <li>To use your mouse and keyboard on it, turn on Can control for this computer in the other computer's Devices page.</li>
          <li>A Mac's ⌘ works as Ctrl on Windows, and the other way round. Alt+Tab and ⌘+Tab stay on the computer you're using.</li>
        </ul>
      </div>
    </div>
  );
}
