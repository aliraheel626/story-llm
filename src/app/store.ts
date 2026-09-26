import { create } from "zustand";

export const SIDEBAR_PANELS = [
  "stories",
  "characters",
  "writingStyle",
  "narratorTools",
  "textModel",
  "imageModel",
  "contextInjection",
  "features",
] as const;

export type SidebarPanelId = (typeof SIDEBAR_PANELS)[number];

interface AppShellState {
  openPanels: Record<SidebarPanelId, boolean>;
  togglePanel: (id: SidebarPanelId) => void;
  sidebarOpen: boolean;
  toggleSidebar: () => void;
}

const SIDEBAR_STORAGE_KEY = "story-llm.sidebarOpen";

function initialSidebarOpen(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_STORAGE_KEY) !== "false";
  } catch {
    return true;
  }
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
    features: false,
  },
  togglePanel: (id) =>
    set((s) => ({ openPanels: { ...s.openPanels, [id]: !s.openPanels[id] } })),
  sidebarOpen: initialSidebarOpen(),
  toggleSidebar: () => set((state) => {
    const sidebarOpen = !state.sidebarOpen;
    try {
      localStorage.setItem(SIDEBAR_STORAGE_KEY, String(sidebarOpen));
    } catch {
      // The in-memory toggle still works when storage is unavailable.
    }
    return { sidebarOpen };
  }),
}));
