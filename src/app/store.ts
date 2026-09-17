import { create } from "zustand";
import { storiesApi } from "../features/stories/api";
import type { Story } from "../shared/types";
import { useDicerollStore } from "../features/dicerolls/store";

export const SIDEBAR_PANELS = [
  "stories",
  "characters",
  "world",
  "attributes",
  "textModel",
  "imageModel",
  "narratorMemory",
  "features",
] as const;

export type SidebarPanelId = (typeof SIDEBAR_PANELS)[number];

interface AppState {
  openPanels: Record<SidebarPanelId, boolean>;
  togglePanel: (id: SidebarPanelId) => void;

  stories: Story[];
  activeStoryId: string | null;
  storiesLoading: boolean;
  creatingStory: boolean;
  /** True while composing a brand-new story that isn't persisted yet — the
   *  record and branch are only created on the first submit, ChatGPT-style.
   *  This is why an accidental "+ New story" click leaves no empty entry. */
  draft: boolean;
  loadStories: () => Promise<void>;
  startDraft: () => void;
  createStory: () => Promise<Story>;
  renameStory: (storyId: string, title: string) => Promise<void>;
  deleteStory: (storyId: string) => Promise<void>;
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
    narratorMemory: false,
    features: false,
  },
  togglePanel: (id) =>
    set((s) => ({ openPanels: { ...s.openPanels, [id]: !s.openPanels[id] } })),

  stories: [],
  activeStoryId: null,
  storiesLoading: false,
  creatingStory: false,
  draft: false,
  loadStories: async () => {
    set({ storiesLoading: true });
    try {
      const stories = await storiesApi.list();
      set({ stories, storiesLoading: false });
    } catch (e) {
      console.error("failed to load stories", e);
      set({ storiesLoading: false });
    }
  },
  startDraft: () => {
    set({ activeStoryId: null, draft: true });
    useDicerollStore.getState().resetDraftSettings();
  },
  createStory: async () => {
    const draft = useDicerollStore.getState().draftSettings;
    set({ creatingStory: true });
    try {
      const story = await storiesApi.create(undefined, draft);
      useDicerollStore.getState().promoteDraftSettings(story.id, draft);
      set((s) => ({ stories: [story, ...s.stories], activeStoryId: story.id, draft: false }));
      return story;
    } finally {
      set({ creatingStory: false });
    }
  },
  renameStory: async (storyId: string, title: string) => {
    await storiesApi.rename(storyId, title);
    set((s) => ({ stories: s.stories.map((st) => (st.id === storyId ? { ...st, title } : st)) }));
  },
  deleteStory: async (storyId: string) => {
    await storiesApi.delete(storyId);
    set((s) => ({
      stories: s.stories.filter((st) => st.id !== storyId),
      activeStoryId: s.activeStoryId === storyId ? null : s.activeStoryId,
    }));
  },
  applyStoryTitle: (storyId: string, title: string) => {
    set((s) => ({ stories: s.stories.map((st) => (st.id === storyId ? { ...st, title } : st)) }));
  },
  setActiveStory: (id: string) => {
    if (get().activeStoryId !== id || get().draft) {
      set({ activeStoryId: id, draft: false });
      useDicerollStore.getState().resetDraftSettings();
    }
  },
}));
