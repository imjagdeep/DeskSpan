// DeskSpan's menus: File, Edit, View, Share, Help.

import { exit } from "@tauri-apps/plugin-process";
import { useEffect, useMemo, useState } from "react";
import { PAGES, type Page } from "./components/Sidebar";
import { api, type LabTool } from "./lib/api";
import { refreshHub, useHub } from "./lib/hub";
import { setTheme, toastError, useTheme, type Theme } from "./lib/store";
import { checkAndDownload } from "./lib/updates";
import type { MenuDef } from "./shell/menu";
import type { MenuIcon } from "./shell/menuIcons";
import {
  editCommand,
  isCompactSidebar,
  openShellDialog,
  pasteText,
  setZoom,
  textFieldFocused,
  toggleFullscreen,
  toggleSidebar,
  useCompactSidebar,
  zoomStep,
} from "./shell/shell";

const PAGE_ICONS: Partial<Record<Page, MenuIcon>> = { share: "screen", devices: "devices", account: "user", settings: "settings" };
const LAB_NAMES: Record<string, string> = { deskzero: "DeskZero", deskmedic: "DeskMedic", deskfolio: "DeskFolio", deskshot: "DeskShot" };

export function useAppMenus(page: Page, go: (p: Page) => void): MenuDef[] {
  const theme = useTheme();
  const compact = useCompactSidebar();
  const hub = useHub();
  const running = hub?.running ?? false;
  const [lab, setLab] = useState<LabTool[]>([]);
  useEffect(() => {
    api.desklabTools().then(setLab).catch(toastError);
  }, []);

  return useMemo(() => {
    const themeItem = (t: Theme, label: string) => ({
      id: `theme-${t}`,
      label,
      checked: () => theme === t,
      run: () => setTheme(t),
    });
    const hubDo = (f: () => Promise<void>) => () => f().then(refreshHub);

    const file: MenuDef = {
      label: "File",
      items: [
        { id: "account", icon: "user", label: "DeskLab account…", run: () => go("account") },
        { id: "settings", icon: "settings", label: "Settings…", accel: "Mod+,", run: () => go("settings") },
        "-",
        // Closing the window keeps DeskSpan in the tray; this quits for real.
        { id: "exit", icon: "exit", label: "Exit DeskSpan", run: () => exit(0) },
      ],
    };

    const edit: MenuDef = {
      label: "Edit",
      items: [
        { id: "undo", icon: "undo", label: "Undo", accel: "Mod+Z", enabled: textFieldFocused, run: () => editCommand("undo") },
        { id: "redo", icon: "redo", label: "Redo", accel: "Mod+Y", enabled: textFieldFocused, run: () => editCommand("redo") },
        "-",
        { id: "cut", icon: "cut", label: "Cut", accel: "Mod+X", native: "cut", hidden: true, run: () => editCommand("cut") },
        { id: "copy", icon: "copy", label: "Copy", accel: "Mod+C", native: "copy", hidden: true, run: () => editCommand("copy") },
        { id: "paste", icon: "paste", label: "Paste", accel: "Mod+V", native: "paste", hidden: true, run: pasteText },
        { id: "select-all", icon: "select", label: "Select all", accel: "Mod+A", native: "selectAll", hidden: true, run: () => editCommand("selectAll") },
      ],
    };

    const view: MenuDef = {
      label: "View",
      items: [
        ...PAGES.map((p, i) => ({ id: `go-${p.page}`, icon: PAGE_ICONS[p.page], label: p.label, accel: `Mod+${i + 1}`, checked: () => page === p.page, run: () => go(p.page) })),
        "-",
        { id: "sidebar", icon: "sidebar", label: "Compact sidebar", accel: "Mod+B", checked: () => isCompactSidebar(), run: toggleSidebar },
        { id: "zoom-in", icon: "zoom-in", label: "Zoom in", accel: "Mod+=", run: () => zoomStep(1) },
        { id: "zoom-out", icon: "zoom-out", label: "Zoom out", accel: "Mod+-", run: () => zoomStep(-1) },
        { id: "zoom-reset", icon: "actual-size", label: "Actual size", accel: "Mod+0", run: () => setZoom(1) },
        "-",
        { label: "Theme", icon: "theme", items: [themeItem("system", "Match system"), themeItem("light", "Light"), themeItem("dark", "Dark")] },
        { id: "fullscreen", icon: "fullscreen", label: "Full screen", accel: "F11", run: toggleFullscreen },
      ],
    };

    const share: MenuDef = {
      label: "Share",
      items: [
        { id: "share-start", icon: "play", label: "Start sharing", accel: "Mod+Shift+S", enabled: () => !running, run: hubDo(api.startSharing) },
        { id: "share-stop", icon: "stop", label: "Stop sharing", accel: "Mod+Shift+X", enabled: () => running, run: hubDo(api.stopSharing) },
        { id: "share-code", icon: "update", label: "New code", enabled: () => running, run: hubDo(api.newPairing) },
        "-",
        { id: "share-devices", icon: "devices", label: "Devices…", run: () => go("devices") },
      ],
    };

    const help: MenuDef = {
      label: "Help",
      items: [
        { id: "palette", icon: "command", label: "Search commands…", accel: "Mod+K", run: () => openShellDialog("palette") },
        { id: "shortcuts", icon: "keyboard", label: "Keyboard shortcuts", accel: "F1", run: () => openShellDialog("shortcuts") },
        "-",
        { id: "updates", icon: "update", label: "Check for updates…", run: () => (go("settings"), void checkAndDownload(true)) },
        { id: "whats-new", icon: "news", label: "What's new", run: () => api.openLink("releases") },
        { id: "report", icon: "bug", label: "Report a problem…", run: () => api.openLink("issues") },
        {
          label: "More DeskLab tools",
          icon: "apps",
          items: lab.map((t) => ({
            id: `lab-${t.id}`,
            label: t.installed ? `Open ${LAB_NAMES[t.id] ?? t.id}` : `Get ${LAB_NAMES[t.id] ?? t.id}`,
            run: () => (t.installed ? api.openTool(t.id) : api.openLink(t.id)),
          })),
        },
        "-",
        { id: "about", icon: "info", label: "About DeskSpan", run: () => openShellDialog("about") },
      ],
    };
    return [file, edit, view, share, help];
    // `compact` re-creates the menus so the check mark stays current.
  }, [page, go, theme, lab, compact, running]);
}
