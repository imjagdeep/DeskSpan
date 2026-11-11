// Typed wrappers over the Rust commands. Errors arrive as plain strings.

import { invoke } from "@tauri-apps/api/core";

export interface AppInfo {
  version: string;
  platform: string;
  /** This computer's name, as receivers see it. */
  host_name: string;
}

export interface Settings {
  port: number;
  /** Off: home/office networks and this computer only. */
  listen_everywhere: boolean;
  start_on_launch: boolean;
  autostart_applied: boolean;
  /** Send the computer's sound to the devices. */
  sound: boolean;
}

export interface ScreenInfo {
  w: number;
  h: number;
  dpr: number;
}

export interface SessionView {
  session: number;
  device_id: string;
  name: string;
  address: string;
  /** Unix seconds */
  since: number;
  screen: ScreenInfo | null;
  /** Using this computer's screen (not just connected) */
  screen_on: boolean;
  /** Connected by USB cable */
  via_usb: boolean;
}

export interface RequestView {
  id: string;
  name: string;
  address: string;
}

export interface UsbPhone {
  serial: string;
  model: string;
  /** "ready", "allow" (tap Allow on the phone), "offline" */
  state: "ready" | "allow" | "offline";
}

export interface UsbView {
  setup: "off" | "preparing" | "ready" | "error";
  error: string | null;
  phones: UsbPhone[];
}

export interface HubStatus {
  running: boolean;
  port: number;
  urls: string[];
  pair_url: string | null;
  code: string | null;
  qr_svg: string | null;
  expires_in: number;
  sessions: SessionView[];
  requests: RequestView[];
  usb: UsbView;
}

export interface VideoStatus {
  state: "idle" | "preparing" | "streaming" | "error";
  width: number;
  height: number;
  fps: number;
  error: string | null;
}

export interface Device {
  id: string;
  name: string;
  /** Unix seconds */
  added: number;
  last_seen: number;
  control: boolean;
  /** Extend (its own extra display) or mirror the main screen */
  extend: boolean;
}

export type LabToolId = "deskzero" | "deskmedic" | "deskfolio" | "deskshot" | "deskspan";
export interface LabTool {
  id: LabToolId;
  installed: boolean;
}

/** One screen on the computer (Displays page). */
export interface Screen {
  id: string;
  name: string;
  /** "HDMI", "DisplayPort", "USB-C", "VGA", "DVI", "Built-in", "Wireless", "DeskSpan", "Virtual", "External" */
  connection: string;
  x: number;
  y: number;
  w: number;
  h: number;
  main: boolean;
  mirror_of: string | null;
  deskspan: boolean;
}

export interface ScreenLayout {
  screens: Screen[];
  can_arrange: boolean;
  can_mirror: boolean;
  mode: "extend" | "duplicate";
}

export const api = {
  screensLayout: () => invoke<ScreenLayout>("screens_layout"),
  screensArrange: (places: { id: string; x: number; y: number }[]) => invoke<void>("screens_arrange", { places }),
  screensSetMain: (id: string) => invoke<void>("screens_set_main", { id }),
  screensSetMode: (duplicate: boolean) => invoke<void>("screens_set_mode", { duplicate }),

  appInfo: () => invoke<AppInfo>("app_info"),
  getSettings: () => invoke<Settings>("get_settings"),
  /** Saved (and tidied) settings come back. */
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),

  startSharing: () => invoke<void>("start_sharing"),
  stopSharing: () => invoke<void>("stop_sharing"),
  hubStatus: () => invoke<HubStatus>("hub_status"),
  videoStatus: () => invoke<VideoStatus>("video_status"),
  /** A fresh QR link and code; the old ones stop working. */
  newPairing: () => invoke<void>("new_pairing"),
  /** Android by USB cable: find or download Google's adb, then watch the cable. */
  usbSetup: () => invoke<void>("usb_setup"),
  usbOff: () => invoke<void>("usb_off"),
  /** Windows: the virtual display driver for extending. */
  extendDriverStatus: () => invoke<"installed" | "missing" | "not-needed">("extend_driver_status"),
  installExtendDriver: () => invoke<void>("install_extend_driver"),
  answerRequest: (id: string, allow: boolean, control: boolean) => invoke<void>("answer_request", { id, allow, control }),

  devices: () => invoke<Device[]>("devices_list"),
  removeDevice: (id: string) => invoke<void>("device_remove", { id }),
  setControl: (id: string, control: boolean) => invoke<void>("device_set_control", { id, control }),
  /** Used the next time the device connects. */
  setExtend: (id: string, extend: boolean) => invoke<void>("device_set_extend", { id, extend }),
  renameDevice: (id: string, name: string) => invoke<void>("device_rename", { id, name }),

  openLink: (id: "site" | "repo" | "issues" | "releases" | "developer" | "screen-permission" | LabToolId) =>
    invoke<void>("open_link", { id }),
  /** The other DeskLab tools and whether each is installed here. */
  desklabTools: () => invoke<LabTool[]>("desklab_tools"),
  /** Start an installed DeskLab tool, optionally with a file for it to open. */
  openTool: (id: LabToolId, file?: string) => invoke<void>("open_tool", { id, file: file ?? null }),
  notifyUpdate: (version: string) => invoke<void>("notify_update", { version }),
};

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}
