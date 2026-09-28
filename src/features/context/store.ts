import { create } from "zustand";
import type { TranscriptItem, ContextPreview, ContextSettings } from "../../shared/types";
import { contextApi } from "./api";

interface StoryContextState {
  items: TranscriptItem[] | null; transcriptLoading: boolean; transcriptSaving: boolean; transcriptError: string | null;
  context: ContextSettings | null;
  contextLoading: boolean;
  contextSaving: boolean;
  contextError: string | null;
  noteDraft: string; preview: ContextPreview | null; previewLoading: boolean; previewError: string | null;
}

interface ContextStore {
  stories: Record<string, StoryContextState>;
  loadTranscriptSettings: (storyId: string) => Promise<void>;
  toggleTranscriptItem: (storyId: string, key: string, enabled: boolean) => Promise<void>;
  loadContextSettings: (storyId: string) => Promise<void>; setNoteDraft: (storyId: string, text: string) => void;
  saveContextSettings: (storyId: string, patch: Partial<ContextSettings>) => Promise<void>;
  saveNote: (storyId: string) => Promise<void>;
  loadPreview: (storyId: string) => Promise<void>;
}
const empty = (): StoryContextState => ({
  items: null, transcriptLoading: false, transcriptSaving: false, transcriptError: null,
  context: null, contextLoading: false, contextSaving: false, contextError: null,
  noteDraft: "", preview: null, previewLoading: false, previewError: null,
});

export const useContextStore = create<ContextStore>((set, get) => {
  const patch = (storyId: string, change: Partial<StoryContextState>) =>
    set((state) => ({ stories: { ...state.stories, [storyId]: { ...(state.stories[storyId] ?? empty()), ...change } } }));
  const previewVersions = new Map<string, number>();
  const invalidatePreview = (storyId: string) => {
    previewVersions.set(storyId, (previewVersions.get(storyId) ?? 0) + 1);
    patch(storyId, { preview: null, previewLoading: false, previewError: null });
  };
  const persistContext = async (storyId: string, settings: ContextSettings, note?: string) => {
    const current = get().stories[storyId];
    if (!current?.context || current.contextSaving) return;
    const previous = current.context;
    patch(storyId, { context: settings, contextSaving: true, contextError: null });
    invalidatePreview(storyId);
    try {
      await contextApi.save(storyId, settings);
      const confirmed = { ...settings, author_note: settings.author_note.trim() };
      patch(storyId, { context: confirmed });
      if (note !== undefined && get().stories[storyId]?.noteDraft === note) {
        patch(storyId, { noteDraft: confirmed.author_note });
      }
    } catch (error) { patch(storyId, { context: previous, contextError: String(error) }); throw error; }
    finally { patch(storyId, { contextSaving: false }); }
  };
  return {
    stories: {},
    loadTranscriptSettings: async (storyId) => {
      const current = get().stories[storyId];
      if (current?.items || current?.transcriptLoading) return;
      patch(storyId, { transcriptLoading: true, transcriptError: null });
      try {
        patch(storyId, { items: await contextApi.getTranscript(storyId) });
      } catch (error) { patch(storyId, { transcriptError: String(error) }); }
      finally { patch(storyId, { transcriptLoading: false }); }
    },
    toggleTranscriptItem: async (storyId, key, enabled) => {
      const current = get().stories[storyId];
      if (!current?.items || current.transcriptSaving || !current.items.some((item) => item.key === key)) return;
      const previous = current.items;
      const items = previous.map((item) => item.key === key ? { ...item, enabled } : item);
      patch(storyId, { items, transcriptSaving: true, transcriptError: null });
      invalidatePreview(storyId);
      try {
        const include = Object.fromEntries(items.map((item) => [item.key, item.enabled]));
        patch(storyId, { items: await contextApi.saveTranscript(storyId, include) });
      } catch (error) { patch(storyId, { items: previous, transcriptError: String(error) }); throw error; }
      finally { patch(storyId, { transcriptSaving: false }); }
    },
    loadContextSettings: async (storyId) => {
      const current = get().stories[storyId];
      if (current?.context || current?.contextLoading) return;
      patch(storyId, { contextLoading: true, contextError: null });
      try {
        const context = await contextApi.get(storyId);
        patch(storyId, { context, noteDraft: context.author_note });
      } catch (error) { patch(storyId, { contextError: String(error) }); }
      finally { patch(storyId, { contextLoading: false }); }
    },
    setNoteDraft: (storyId, noteDraft) => patch(storyId, { noteDraft }),
    saveContextSettings: async (storyId, change) => {
      const context = get().stories[storyId]?.context;
      if (context) await persistContext(storyId, { ...context, ...change });
    },
    saveNote: async (storyId) => {
      const current = get().stories[storyId];
      if (current?.context) {
        await persistContext(storyId, { ...current.context, author_note: current.noteDraft }, current.noteDraft);
      }
    },
    loadPreview: async (storyId) => {
      const current = get().stories[storyId];
      if (!current?.items || current.transcriptSaving || current.contextSaving) return;
      const version = (previewVersions.get(storyId) ?? 0) + 1;
      previewVersions.set(storyId, version);
      patch(storyId, { previewLoading: true, previewError: null, preview: null });
      try {
        const preview = await contextApi.preview(storyId);
        if (previewVersions.get(storyId) === version) patch(storyId, { preview });
      } catch (error) { if (previewVersions.get(storyId) === version) patch(storyId, { previewError: String(error) }); }
      finally { if (previewVersions.get(storyId) === version) patch(storyId, { previewLoading: false }); }
    },
  };
});
