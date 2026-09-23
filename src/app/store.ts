import { create } from "zustand";

export const SIDEBAR_PANELS = [
  "stories",
  "characters",
  "writingStyle",
  "narratorTools",
  "textModel",
  "imageModel",
  "contextInjection",
  "ledgerRetention",
  "features",
] as const;

export type SidebarPanelId = (typeof SIDEBAR_PANELS)[number];

interface AppShellState {
  openPanels: Record<SidebarPanelId, boolean>;
  togglePanel: (id: SidebarPanelId) => void;
}

export const useAppShellStore = create<AppShellState>((set) => ({
  openPanels: {
    stories: true,
    characters: false,
    writingStyle: false,
    narratorTools: false,
    textModel: false,
    imageModel: false,
    contextInjection: false,
    ledgerRetention: false,
    features: false,
  },
  togglePanel: (id) =>
    set((s) => ({ openPanels: { ...s.openPanels, [id]: !s.openPanels[id] } })),
}));
