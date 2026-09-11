import { create } from "zustand";
import { commands } from "../lib/commands";
import type { Story } from "../lib/types";

export const SIDEBAR_PANELS = [
  "stories",
  "characters",
  "world",
  "attributes",
  "textModel",
  "imageModel",
  "features",
] as const;

export type SidebarPanelId = (typeof SIDEBAR_PANELS)[number];

interface AppState {
  openPanels: Record<SidebarPanelId, boolean>;
  togglePanel: (id: SidebarPanelId) => void;

  stories: Story[];
  activeStoryId: string | null;
  storiesLoading: boolean;
  loadStories: () => Promise<void>;
  createStory: (title: string) => Promise<void>;
  setActiveStory: (id: string) => void;
}

export const useAppStore = create<AppState>((set, get) => ({
  openPanels: {
    stories: true,
    characters: false,
    world: false,
    attributes: false,
    textModel: false,
    imageModel: false,
    features: false,
  },
  togglePanel: (id) =>
    set((s) => ({ openPanels: { ...s.openPanels, [id]: !s.openPanels[id] } })),

  stories: [],
  activeStoryId: null,
  storiesLoading: false,
  loadStories: async () => {
    set({ storiesLoading: true });
    try {
      const stories = await commands.listStories();
      set({ stories, storiesLoading: false });
    } catch (e) {
      console.error("failed to load stories", e);
      set({ storiesLoading: false });
    }
  },
  createStory: async (title: string) => {
    const story = await commands.createStory(title);
    set((s) => ({ stories: [story, ...s.stories], activeStoryId: story.id }));
  },
  setActiveStory: (id: string) => {
    if (get().activeStoryId !== id) set({ activeStoryId: id });
  },
}));
