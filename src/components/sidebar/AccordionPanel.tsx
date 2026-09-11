import { ReactNode } from "react";

interface AccordionPanelProps {
  title: string;
  open: boolean;
  onToggle: () => void;
  headerAction?: ReactNode;
  children: ReactNode;
}

export function AccordionPanel({ title, open, onToggle, headerAction, children }: AccordionPanelProps) {
  return (
    <div className="border-b border-border">
      <div className="flex items-center justify-between px-3 py-2">
        <button
          onClick={onToggle}
          className="flex flex-1 items-center gap-2 text-left text-xs font-semibold uppercase tracking-wide text-muted hover:text-text transition-colors"
          aria-expanded={open}
        >
          <svg
            width="10"
            height="10"
            viewBox="0 0 10 10"
            className={`shrink-0 transition-transform duration-150 ${open ? "rotate-90" : ""}`}
            style={{ transitionDuration: "var(--dungeon-motion, 150ms)" }}
          >
            <path d="M2 1 L8 5 L2 9" stroke="currentColor" strokeWidth="1.5" fill="none" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          {title}
        </button>
        {headerAction}
      </div>
      {open && <div className="px-3 pb-3 animate-fade-in">{children}</div>}
    </div>
  );
}
