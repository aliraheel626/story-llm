import { create } from "zustand";
import type { Entity } from "../../shared/types";
import { charactersApi } from "./api";

interface CharacterState {
  entitiesByStory: Record<string, Entity[]>;
  loading: boolean;

  loadCharacters: (storyId: string, branchId: string) => Promise<void>;
  createCharacter: (storyId: string, branchId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  updateCharacter: (storyId: string, branchId: string, entityId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  deleteCharacter: (storyId: string, branchId: string, entityId: string) => Promise<void>;
}

export const useCharacterStore = create<CharacterState>((set) => ({
  entitiesByStory: {},
  loading: false,

  loadCharacters: async (storyId: string, branchId: string) => {
    set({ loading: true });
    try {
      const entities = await charactersApi.list(storyId, branchId);
      set((s) => ({ entitiesByStory: { ...s.entitiesByStory, [storyId]: entities }, loading: false }));
    } catch (e) {
      console.error("failed to load characters", e);
      set({ loading: false });
    }
  },

  createCharacter: async (storyId: string, branchId: string, name: string, appearanceAnchor?: string) => {
    const entity = await charactersApi.create(storyId, branchId, name, appearanceAnchor);
    set((s) => ({ entitiesByStory: { ...s.entitiesByStory, [storyId]: [...(s.entitiesByStory[storyId] ?? []), entity] } }));
  },

  updateCharacter: async (storyId: string, branchId: string, entityId: string, name: string, appearanceAnchor?: string) => {
    const entity = await charactersApi.update(branchId, entityId, name, appearanceAnchor);
    set((s) => ({
      entitiesByStory: {
        ...s.entitiesByStory,
        [storyId]: (s.entitiesByStory[storyId] ?? []).map((e) => (e.id === entityId ? entity : e)),
      },
    }));
  },

  deleteCharacter: async (storyId: string, branchId: string, entityId: string) => {
    await charactersApi.delete(branchId, entityId);
    set((s) => ({
      entitiesByStory: { ...s.entitiesByStory, [storyId]: (s.entitiesByStory[storyId] ?? []).filter((e) => e.id !== entityId) },
    }));
  },
}));
