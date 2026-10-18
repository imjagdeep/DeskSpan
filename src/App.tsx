import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import { useAppMenus } from "./appMenu";
import logo from "./assets/logo.png";
import { Toasts } from "./components/Common";
import { PAGES, Sidebar, type Page } from "./components/Sidebar";
import { UpdateBanner } from "./components/UpdateBanner";
import { api } from "./lib/api";
import { watchHub } from "./lib/hub";
import { refreshAppInfo, toast, toastError, useAppInfo, useAutoUpdate, useTheme } from "./lib/store";
import { checkAndDownload, dailyCheckDue } from "./lib/updates";
import { Devices } from "./pages/Devices";
import { Share } from "./pages/Share";
import { SettingsPage } from "./pages/Settings";
import { AccountPage } from "./account/AccountPage";
import { ShellDialogs } from "./shell/Dialogs";
import { TopBar } from "./shell/TopBar";
import { applySavedZoom } from "./shell/shell";

export default function App() {
  const [page, setPageRaw] = useState<Page>("share");
  const setPage = useCallback((p: Page) => setPageRaw(p), []);
  const menus = useAppMenus(page, setPage);
  const info = useAppInfo();
  const theme = useTheme();
  const autoUpdate = useAutoUpdate();
  const content = useRef<HTMLElement>(null);

  useEffect(() => {
    void refreshAppInfo();
    void applySavedZoom().catch(() => undefined);
    const unHub = watchHub();
    // From the tray menu.
    const unNav = listen<string>("navigate", (e) => {
      const target = PAGES.find((p) => p.page === e.payload);
      if (target) setPageRaw(target.page);
    });
    return () => {
      unHub();
      void unNav.then((f) => f());
    };
  }, []);

  // Automatic updates: soon after start, then daily while DeskSpan keeps
  // running in the tray. Same as the sister apps.
  useEffect(() => {
    if (!autoUpdate) return;
    const run = () => {
      if (!dailyCheckDue()) return;
      void checkAndDownload(false).then((v) => {
        if (v) void api.notifyUpdate(v).catch(() => undefined);
      });
    };
    const first = window.setTimeout(run, 30_000);
    const every = window.setInterval(run, 3 * 3600 * 1000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(every);
    };
  }, [autoUpdate]);

  // "system" leaves the attribute off so the CSS media query decides.
  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  // The window title names the page, like any desktop app.
  useEffect(() => {
    const where = PAGES.find((p) => p.page === page)?.label ?? "";
    void getCurrentWindow()
      .setTitle(page === "share" ? "DeskSpan" : `${where} — DeskSpan`)
      .catch(() => undefined);
    content.current?.scrollTo(0, 0);
  }, [page]);

  return (
    <div className="app-frame">
      <TopBar appName="DeskSpan" logo={logo} title={PAGES.find((p) => p.page === page)?.label} menus={menus} onError={toastError} />
      <div className="app">
        <Sidebar page={page} onNavigate={setPage} />
        <main className="content" ref={content}>
          {page === "share" && <Share />}
          {page === "devices" && <Devices />}
          {page === "account" && <AccountPage app="DeskSpan" toast={toast} />}
          {page === "settings" && <SettingsPage />}
        </main>
      </div>
      <ShellDialogs
        menus={menus}
        onError={toastError}
        about={{
          name: "DeskSpan",
          logo,
          version: info?.version ?? "",
          tagline: "Use a phone, tablet or another computer as a second screen: mirror or extend, over your own network. Free and private.",
          links: [
            { label: "Website", open: () => void api.openLink("site").catch(toastError) },
            { label: "Source code", open: () => void api.openLink("repo").catch(toastError) },
            { label: "What's new", open: () => void api.openLink("releases").catch(toastError) },
            { label: "Report a problem", open: () => void api.openLink("issues").catch(toastError) },
          ],
        }}
      />
      <UpdateBanner app="DeskSpan" />
      <Toasts />
    </div>
  );
}
