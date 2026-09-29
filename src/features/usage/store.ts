import { create } from "zustand";
import type { StoryUsage } from "../../shared/types";
import { usageApi } from "./api";

interface UsageStore {
  byStory: Record<string, StoryUsage | undefined>;
  load: (storyId: string) => Promise<void>;
}

const requestVersions = new Map<string, number>();

export const useUsageStore = create<UsageStore>((set) => ({
  byStory: {},
  load: async (storyId) => {
    const version = (requestVersions.get(storyId) ?? 0) + 1;
    requestVersions.set(storyId, version);
    try {
      const usage = await usageApi.get(storyId);
      if (requestVersions.get(storyId) === version) {
        set((state) => ({ byStory: { ...state.byStory, [storyId]: usage } }));
      }
    } catch (error) {
      console.warn("Could not load story usage", error);
    }
  },
}));

export const formatUsd = (amount: number) => amount === 0 ? "$0.00" : `$${amount.toFixed(amount < 1 ? 4 : 2)}`;
export const formatTokens = (count: number) => new Intl.NumberFormat("en", {
  notation: "compact", maximumFractionDigits: 1,
}).format(count);
