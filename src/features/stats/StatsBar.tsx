import { useEffect } from "react";
import { useStoryStore } from "../story/store";
import { formatTokens, formatUsd, useStatsStore } from "./store";

export function StatsBar({ storyId }: { storyId: string }) {
  const busy = useStoryStore((state) => !!state.bundles[storyId]?.streaming);
  const stats = useStatsStore((state) => state.byStory[storyId]);
  const load = useStatsStore((state) => state.load);

  useEffect(() => { if (!busy) load(storyId); }, [storyId, busy, load]);

  const cachedPercent = stats?.input_tokens ? Math.round(stats.cached_input_tokens / stats.input_tokens * 100) : 0;
  const totalTooltip = stats?.since
    ? `Text + images since ${new Date(stats.since).toLocaleDateString()}`
    : "No usage recorded yet";
  const unpriced = stats?.unpriced_calls ?? 0;
  const totalTitle = unpriced > 0
    ? `${totalTooltip}. ${unpriced} calls reported no cost (Ollama, Nous Portal, or a request that failed or timed out before reporting its cost) and aren't included`
    : totalTooltip;

  return (
    <div className="flex shrink-0 items-center justify-end gap-2 overflow-x-auto whitespace-nowrap border-b border-border px-3 py-3 text-xs text-muted tabular-nums sm:gap-4 sm:px-6">
      <span title="Narration, context summaries and titles">Text {stats ? formatUsd(stats.text_cost_usd) : "—"}</span>
      <span title={stats ? `${stats.image_count} scene images` : "Scene images"}>
        Images {stats ? `${formatUsd(stats.image_cost_usd)} (${stats.image_count})` : "—"}
      </span>
      <span className="hidden md:inline" title={stats
        ? `${stats.cached_input_tokens.toLocaleString()} of ${stats.input_tokens.toLocaleString()} input tokens (${cachedPercent}%) were read from the provider's cache`
        : "Cached input tokens"}>
        Cached input {stats ? formatTokens(stats.cached_input_tokens) : "—"}
      </span>
      <span className="hidden md:inline" title="Input tokens written to the provider's cache">
        Cache write {stats ? formatTokens(stats.cache_write_tokens) : "—"}
      </span>
      <span className="font-medium text-text" title={totalTitle}>
        Total {stats ? formatUsd(stats.total_cost_usd) : "—"}{unpriced > 0 ? "*" : ""}
      </span>
    </div>
  );
}
