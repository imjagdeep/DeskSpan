import { useToasts } from "../lib/store";
import type { IconType } from "./Icons";

export function Toasts() {
  const toasts = useToasts();
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={t.error ? "toast error" : "toast"}>
          {t.text}
        </div>
      ))}
    </div>
  );
}

export function PageHeader({ title, subtitle, children }: { title: string; subtitle?: string; children?: React.ReactNode }) {
  return (
    <header className="page-header">
      <div className="page-title">
        <h1>{title}</h1>
        {subtitle && <div className="page-subtitle">{subtitle}</div>}
      </div>
      <div className="page-actions">{children}</div>
    </header>
  );
}

/** Centred empty state with an icon, a line of text and an optional action. */
export function Empty({ Icon, title, text, children }: { Icon: IconType; title: string; text?: string; children?: React.ReactNode }) {
  return (
    <div className="empty empty-big">
      <Icon size={34} />
      <div className="empty-title">{title}</div>
      {text && <div className="muted small">{text}</div>}
      {children && <div className="empty-actions">{children}</div>}
    </div>
  );
}

/** Asks for a PDF's password when doc.ts needs one. */
