import { create } from "zustand";
import type { StoryStats } from "../../shared/types";
import { statsApi } from "./api";

interface StatsStore {
  byStory: Record<string, StoryStats | undefined>;
  load: (storyId: string) => Promise<void>;
}

const requestVersions = new Map<string, number>();

export const useStatsStore = create<StatsStore>((set) => ({
  byStory: {},
  load: async (storyId) => {
    const version = (requestVersions.get(storyId) ?? 0) + 1;
    requestVersions.set(storyId, version);
    try {
      const stats = await statsApi.get(storyId);
      if (requestVersions.get(storyId) === version) {
        set((state) => ({ byStory: { ...state.byStory, [storyId]: stats } }));
      }
    } catch (error) {
      console.warn("Could not load story stats", error);
    }
  },
}));

export const formatUsd = (amount: number) => amount === 0 ? "$0.00" : `$${amount.toFixed(amount < 1 ? 4 : 2)}`;
export const formatTokens = (count: number) => new Intl.NumberFormat("en", {
  notation: "compact", maximumFractionDigits: 1,
}).format(count);
