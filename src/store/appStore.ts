import { create } from "zustand";
import { commands } from "../lib/commands";
import type { Story } from "../lib/types";
import { useMechanicsStore } from "./mechanicsStore";

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
  /** True while composing a brand-new story that isn't persisted yet — the
   *  record and branch are only created on the first submit, ChatGPT-style.
   *  This is why an accidental "+ New story" click leaves no empty entry. */
  draft: boolean;
  loadStories: () => Promise<void>;
  startDraft: () => void;
  createStory: () => Promise<Story>;
  renameStory: (storyId: string, title: string) => Promise<void>;
  applyStoryTitle: (storyId: string, title: string) => void;
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
  draft: false,
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
  startDraft: () => {
    set({ activeStoryId: null, draft: true });
    useMechanicsStore.getState().resetDraftSettings();
  },
  createStory: async () => {
    const draft = useMechanicsStore.getState().draftSettings;
    const story = await commands.createStory(undefined, draft);
    useMechanicsStore.getState().promoteDraftSettings(story.id);
    set((s) => ({ stories: [story, ...s.stories], activeStoryId: story.id, draft: false }));
    return story;
  },
  renameStory: async (storyId: string, title: string) => {
    await commands.renameStory(storyId, title);
    set((s) => ({ stories: s.stories.map((st) => (st.id === storyId ? { ...st, title } : st)) }));
  },
  applyStoryTitle: (storyId: string, title: string) => {
    set((s) => ({ stories: s.stories.map((st) => (st.id === storyId ? { ...st, title } : st)) }));
  },
  setActiveStory: (id: string) => {
    if (get().activeStoryId !== id || get().draft) {
      set({ activeStoryId: id, draft: false });
      useMechanicsStore.getState().resetDraftSettings();
    }
  },
}));
