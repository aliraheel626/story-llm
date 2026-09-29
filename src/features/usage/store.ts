import { create } from "zustand";
import type { ImageCost, StoryUsage, TurnCost } from "../../shared/types";
import { usageApi } from "./api";

interface UsageStore {
  byStory: Record<string, StoryUsage | undefined>;
  breakdownByStory: Record<string, { turns: Record<string, TurnCost>; images: Record<string, ImageCost> } | undefined>;
  load: (storyId: string) => Promise<void>;
}

const requestVersions = new Map<string, number>();

export const useUsageStore = create<UsageStore>((set) => ({
  byStory: {},
  breakdownByStory: {},
  load: async (storyId) => {
    const version = (requestVersions.get(storyId) ?? 0) + 1;
    requestVersions.set(storyId, version);
    try {
      const [usage, breakdown] = await Promise.all([usageApi.get(storyId), usageApi.breakdown(storyId)]);
      if (requestVersions.get(storyId) === version) {
        set((state) => ({
          byStory: { ...state.byStory, [storyId]: usage },
          breakdownByStory: {
            ...state.breakdownByStory,
            [storyId]: {
              turns: Object.fromEntries(breakdown.turns.map((turn) => [turn.turn_id, turn])),
              images: Object.fromEntries(breakdown.images.map((image) => [image.asset_id, image])),
            },
          },
        }));
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
export const formatSeconds = (ms: number | null) => ms === null ? "—" : `${(ms / 1000).toFixed(1)} s`;
