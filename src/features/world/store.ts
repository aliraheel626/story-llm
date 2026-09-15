import { create } from "zustand";
import { worldApi } from "./api";

interface AuthorNoteState {
  noteByStory: Record<string, string>;
  loading: boolean;
  saving: boolean;
  load: (storyId: string) => Promise<void>;
  save: (storyId: string, branchId: string, note: string) => Promise<void>;
}

export const useAuthorNoteStore = create<AuthorNoteState>((set) => ({
  noteByStory: {},
  loading: false,
  saving: false,
  load: async (storyId: string) => {
    set({ loading: true });
    try {
      const note = await worldApi.getAuthorNote(storyId);
      set((s) => ({ noteByStory: { ...s.noteByStory, [storyId]: note }, loading: false }));
    } catch (e) {
      console.error("failed to load author's note", e);
      set({ loading: false });
    }
  },
  save: async (storyId: string, branchId: string, note: string) => {
    set({ saving: true });
    try {
      await worldApi.saveAuthorNote(storyId, branchId, note);
      set((s) => ({ noteByStory: { ...s.noteByStory, [storyId]: note.trim() }, saving: false }));
    } catch (e) {
      set({ saving: false });
      throw e;
    }
  },
}));
