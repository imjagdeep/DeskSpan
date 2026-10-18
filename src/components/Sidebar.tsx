import logo from "../assets/logo.png";
import { useHub } from "../lib/hub";
import { SideNav, type NavGroup } from "../shell/SideNav";
import { AccountIcon, DevicesIcon, SettingsIcon, ShareIcon } from "./Icons";

export type Page = "share" | "devices" | "account" | "settings";

/** The pages in sidebar order (the View menu and Ctrl+1… use it too). */
export const PAGES: { page: Page; label: string; group: string }[] = [
  { page: "share", label: "Share screen", group: "Screens" },
  { page: "devices", label: "Devices", group: "Screens" },
  { page: "account", label: "Account", group: "App" },
  { page: "settings", label: "Settings", group: "App" },
];

const ICONS: Record<Page, (p: { size?: number }) => React.ReactElement> = {
  share: ShareIcon,
  devices: DevicesIcon,
  account: AccountIcon,
  settings: SettingsIcon,
};

export function Sidebar({ page, onNavigate }: { page: Page; onNavigate: (p: Page) => void }) {
  const hub = useHub();
  const groups: NavGroup<Page>[] = [];
  for (const p of PAGES) {
    let g = groups.find((x) => x.title === p.group);
    if (!g) groups.push((g = { title: p.group, items: [] }));
    g.items.push({ page: p.page, label: p.label, Icon: ICONS[p.page] });
  }
  const n = hub?.sessions.length ?? 0;
  const state = !hub ? "…" : !hub.running ? "Not sharing" : n === 0 ? "Sharing · no screens yet" : n === 1 ? "Sharing · 1 screen" : `Sharing · ${n} screens`;
  return (
    <SideNav
      appName="DeskSpan"
      logo={logo}
      groups={groups}
      page={page}
      onNavigate={onNavigate}
      footTitle={hub?.running ? "Receivers on your network can connect" : undefined}
      foot={
        <>
          <span className={hub?.running ? "dot on" : "dot"} />
          <span className="ellipsis">{state}</span>
        </>
      }
    />
  );
}
