import { create } from "zustand";
import { commands } from "../lib/commands";

interface AuthorNoteState {
  noteByStory: Record<string, string>;
  loading: boolean;
  saving: boolean;
  load: (storyId: string) => Promise<void>;
  save: (storyId: string, note: string) => Promise<void>;
}

export const useAuthorNoteStore = create<AuthorNoteState>((set) => ({
  noteByStory: {},
  loading: false,
  saving: false,
  load: async (storyId: string) => {
    set({ loading: true });
    try {
      const note = await commands.getAuthorNote(storyId);
      set((s) => ({ noteByStory: { ...s.noteByStory, [storyId]: note }, loading: false }));
    } catch (e) {
      console.error("failed to load author's note", e);
      set({ loading: false });
    }
  },
  save: async (storyId: string, note: string) => {
    set({ saving: true });
    try {
      await commands.saveAuthorNote(storyId, note);
      set((s) => ({ noteByStory: { ...s.noteByStory, [storyId]: note.trim() }, saving: false }));
    } catch (e) {
      set({ saving: false });
      throw e;
    }
  },
}));
