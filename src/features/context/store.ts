import { create } from "zustand";
import type { ContextItem, ContextPreview, InjectionSettings } from "../../shared/types";
import { contextApi } from "./api";

interface StoryContextState {
  items: ContextItem[] | null;
  contextLoading: boolean;
  contextSaving: boolean;
  contextError: string | null;
  injection: InjectionSettings | null;
  injectionLoading: boolean;
  injectionSaving: boolean;
  injectionError: string | null;
  noteDraft: string;
  preview: ContextPreview | null;
  previewLoading: boolean;
  previewError: string | null;
}

interface ContextStore {
  stories: Record<string, StoryContextState>;
  loadContext: (storyId: string) => Promise<void>;
  toggleContext: (storyId: string, key: string, enabled: boolean) => Promise<void>;
  loadInjection: (storyId: string) => Promise<void>;
  setNoteDraft: (storyId: string, text: string) => void;
  saveInjection: (storyId: string, patch: Partial<InjectionSettings>) => Promise<void>;
  saveNote: (storyId: string) => Promise<void>;
  loadPreview: (storyId: string) => Promise<void>;
}

const empty = (): StoryContextState => ({
  items: null, contextLoading: false, contextSaving: false, contextError: null,
  injection: null, injectionLoading: false, injectionSaving: false, injectionError: null,
  noteDraft: "", preview: null, previewLoading: false, previewError: null,
});

export const useContextStore = create<ContextStore>((set, get) => {
  const patch = (storyId: string, change: Partial<StoryContextState>) =>
    set((state) => ({ stories: { ...state.stories, [storyId]: { ...(state.stories[storyId] ?? empty()), ...change } } }));
  const previewVersions = new Map<string, number>();
  const contextSaves = new Map<string, Promise<void>>();
  const injectionSaves = new Map<string, Promise<void>>();
  const confirmedContext = new Map<string, ContextItem[]>();
  const confirmedInjection = new Map<string, InjectionSettings>();
  const invalidatePreview = (storyId: string) => {
    previewVersions.set(storyId, (previewVersions.get(storyId) ?? 0) + 1);
    patch(storyId, { preview: null, previewLoading: false, previewError: null });
  };
  const persistInjection = async (storyId: string, settings: InjectionSettings, note?: string) => {
    const current = get().stories[storyId];
    if (!current?.injection) return;
    patch(storyId, { injection: settings, injectionSaving: true, injectionError: null });
    invalidatePreview(storyId);
    const previous = injectionSaves.get(storyId) ?? Promise.resolve();
    const save = previous.catch(() => undefined).then(async () => {
      await contextApi.saveInjection(storyId, settings);
      const confirmed = { ...settings, author_note: settings.author_note.trim() };
      confirmedInjection.set(storyId, confirmed);
      if (note !== undefined && get().stories[storyId]?.noteDraft === note) {
        patch(storyId, { noteDraft: confirmed.author_note });
      }
      if (injectionSaves.get(storyId) === save) patch(storyId, { injection: confirmed });
    });
    injectionSaves.set(storyId, save);
    try {
      await save;
    } catch (error) {
      if (injectionSaves.get(storyId) === save) {
        patch(storyId, { injection: confirmedInjection.get(storyId) ?? current.injection, injectionError: String(error) });
      }
      throw error;
    } finally {
      if (injectionSaves.get(storyId) === save) {
        injectionSaves.delete(storyId);
        patch(storyId, { injectionSaving: false });
      }
    }
  };

  return {
    stories: {},
    loadContext: async (storyId) => {
      const current = get().stories[storyId];
      if (current?.items || current?.contextLoading) return;
      patch(storyId, { contextLoading: true, contextError: null });
      try {
        const items = await contextApi.get(storyId);
        confirmedContext.set(storyId, items);
        patch(storyId, { items });
      } catch (error) {
        patch(storyId, { contextError: String(error) });
      } finally {
        patch(storyId, { contextLoading: false });
      }
    },
    toggleContext: async (storyId, key, enabled) => {
      const current = get().stories[storyId];
      if (!current?.items || !current.items.some((item) => item.key === key)) return;
      const items = current.items.map((item) => item.key === key ? { ...item, enabled } : item);
      patch(storyId, { items, contextSaving: true, contextError: null });
      invalidatePreview(storyId);
      const previous = contextSaves.get(storyId) ?? Promise.resolve();
      const save = previous.catch(() => undefined).then(async () => {
        const include = Object.fromEntries(items.map((item) => [item.key, item.enabled]));
        const confirmed = await contextApi.save(storyId, include);
        confirmedContext.set(storyId, confirmed);
        if (contextSaves.get(storyId) === save) patch(storyId, { items: confirmed });
      });
      contextSaves.set(storyId, save);
      try {
        await save;
      } catch (error) {
        if (contextSaves.get(storyId) === save) {
          patch(storyId, { items: confirmedContext.get(storyId) ?? current.items, contextError: String(error) });
        }
        throw error;
      } finally {
        if (contextSaves.get(storyId) === save) {
          contextSaves.delete(storyId);
          patch(storyId, { contextSaving: false });
        }
      }
    },
    loadInjection: async (storyId) => {
      const current = get().stories[storyId];
      if (current?.injection || current?.injectionLoading) return;
      patch(storyId, { injectionLoading: true, injectionError: null });
      try {
        const injection = await contextApi.getInjection(storyId);
        confirmedInjection.set(storyId, injection);
        patch(storyId, { injection, noteDraft: injection.author_note });
      } catch (error) {
        patch(storyId, { injectionError: String(error) });
      } finally {
        patch(storyId, { injectionLoading: false });
      }
    },
    setNoteDraft: (storyId, noteDraft) => patch(storyId, { noteDraft }),
    saveInjection: async (storyId, change) => {
      const injection = get().stories[storyId]?.injection;
      if (injection) await persistInjection(storyId, { ...injection, ...change });
    },
    saveNote: async (storyId) => {
      const current = get().stories[storyId];
      if (current?.injection) {
        await persistInjection(storyId, { ...current.injection, author_note: current.noteDraft }, current.noteDraft);
      }
    },
    loadPreview: async (storyId) => {
      if (!get().stories[storyId]?.items) return;
      const version = (previewVersions.get(storyId) ?? 0) + 1;
      previewVersions.set(storyId, version);
      patch(storyId, { previewLoading: true, previewError: null, preview: null });
      try {
        const preview = await contextApi.preview(storyId);
        if (previewVersions.get(storyId) === version) patch(storyId, { preview });
      } catch (error) {
        if (previewVersions.get(storyId) === version) patch(storyId, { previewError: String(error) });
      } finally {
        if (previewVersions.get(storyId) === version) patch(storyId, { previewLoading: false });
      }
    },
  };
});
