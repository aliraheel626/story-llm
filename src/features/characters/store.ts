import { create } from "zustand";
import type { Entity } from "../../shared/types";
import { charactersApi } from "./api";

interface CharacterState {
  entitiesByStory: Record<string, Entity[]>;
  loading: boolean;

  loadCharacters: (storyId: string) => Promise<void>;
  createCharacter: (storyId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  updateCharacter: (storyId: string, entityId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  deleteCharacter: (storyId: string, entityId: string) => Promise<void>;
}

export const useCharacterStore = create<CharacterState>((set) => ({
  entitiesByStory: {},
  loading: false,

  loadCharacters: async (storyId: string) => {
    set({ loading: true });
    try {
      const entities = await charactersApi.list(storyId);
      set((s) => ({ entitiesByStory: { ...s.entitiesByStory, [storyId]: entities }, loading: false }));
    } catch (e) {
      console.error("failed to load characters", e);
      set({ loading: false });
    }
  },

  createCharacter: async (storyId: string, name: string, appearanceAnchor?: string) => {
    const entity = await charactersApi.create(storyId, name, appearanceAnchor);
    set((s) => ({ entitiesByStory: { ...s.entitiesByStory, [storyId]: [...(s.entitiesByStory[storyId] ?? []), entity] } }));
  },

  updateCharacter: async (storyId: string, entityId: string, name: string, appearanceAnchor?: string) => {
    const entity = await charactersApi.update(entityId, name, appearanceAnchor);
    set((s) => ({
      entitiesByStory: {
        ...s.entitiesByStory,
        [storyId]: (s.entitiesByStory[storyId] ?? []).map((e) => (e.id === entityId ? entity : e)),
      },
    }));
  },

  deleteCharacter: async (storyId: string, entityId: string) => {
    await charactersApi.delete(entityId);
    set((s) => ({
      entitiesByStory: { ...s.entitiesByStory, [storyId]: (s.entitiesByStory[storyId] ?? []).filter((e) => e.id !== entityId) },
    }));
  },
}));
