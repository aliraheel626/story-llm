import { useState } from "react";
import { useImageModelStore } from "./imageModelStore";
import { useTextModelStore } from "./textModelStore";
import { useSettingsForm } from "./useSettingsForm";

const STYLE_PRESETS: { label: string; value: string }[] = [
  { label: "Painterly", value: "Digital painting, atmospheric scene illustration." },
  { label: "Anime", value: "Anime style illustration, vibrant colors, clean linework." },
  { label: "Realistic", value: "Photorealistic, cinematic lighting, high detail." },
  { label: "Comic", value: "Comic book art style, bold inked linework, dynamic shading." },
];

/** Models we've measured against the images endpoint. No resolution tier is
 *  ever requested — the model default is its lowest supported tier and the
 *  fastest in every case we probed. */
const IMAGE_MODELS: { slug: string; label: string }[] = [
  { slug: "google/gemini-3.1-flash-lite-image", label: "Nano Banana 2 Lite (fastest)" },
  { slug: "black-forest-labs/flux.2-pro", label: "FLUX.2 Pro (anime-friendly)" },
  { slug: "x-ai/grok-imagine-image-2.0", label: "Grok Imagine 2.0" },
];

export function ImageModelPanel() {
  const textSettings = useTextModelStore((s) => s.settings);

  const [model, setModel] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [narratorImages, setNarratorImages] = useState(true);
  const [style, setStyle] = useState("");
  const [customMode, setCustomMode] = useState(false);
  const { settings, loading, saving, savedNotice, save } = useSettingsForm(useImageModelStore, (next) => {
    setModel(next.model);
    setEnabled(next.enabled);
    setNarratorImages(next.narrator_images);
    setStyle(next.style);
    setCustomMode(!IMAGE_MODELS.some((candidate) => candidate.slug === next.model));
  });

  const onModelSelect = (value: string) => {
    if (value === "__custom__") {
      setCustomMode(true);
      return;
    }
    setCustomMode(false);
    setModel(value);
  };

  const onSave = async () => {
    await save(model.trim(), enabled, style.trim(), narratorImages);
  };

  if (loading && !settings) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  const hasKey = settings?.has_api_key ?? textSettings?.has_api_key ?? false;

  return (
    <div className="flex flex-col gap-2 text-sm">
      <label className="flex items-center gap-2 text-xs text-muted">
        <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} className="accent-accent" />
        Enable image generation
      </label>

      <label className="flex items-center gap-2 text-xs text-muted">
        <input
          type="checkbox"
          checked={narratorImages}
          onChange={(e) => setNarratorImages(e.target.checked)}
          disabled={!enabled}
          className="accent-accent disabled:opacity-50"
        />
        Let the narrator illustrate scenes
      </label>

      <div>
        <label className="block text-xs text-muted mb-1">Model</label>
        <select
          value={customMode ? "__custom__" : model}
          onChange={(e) => onModelSelect(e.target.value)}
          disabled={!enabled}
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text focus:outline-none focus:border-accent disabled:opacity-50"
        >
          {IMAGE_MODELS.map((m) => (
            <option key={m.slug} value={m.slug}>
              {m.label}
            </option>
          ))}
          <option value="__custom__">Custom…</option>
        </select>
        {customMode && (
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="provider/model"
            disabled={!enabled}
            className="mt-1.5 w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent disabled:opacity-50"
          />
        )}
      </div>

      <div>
        <label className="block text-xs text-muted mb-1">Style</label>
        <div className="mb-1.5 flex flex-wrap gap-1">
          {STYLE_PRESETS.map((preset) => (
            <button
              key={preset.label}
              onClick={() => setStyle(preset.value)}
              disabled={!enabled}
              className={`rounded px-2 py-0.5 text-[11px] transition-colors ${
                style === preset.value ? "bg-accent text-bg" : "border border-border bg-bg text-muted hover:text-text"
              } disabled:opacity-50`}
            >
              {preset.label}
            </button>
          ))}
        </div>
        <textarea
          value={style}
          onChange={(e) => setStyle(e.target.value)}
          placeholder="Style prefix applied to every generated image..."
          rows={2}
          disabled={!enabled}
          className="w-full resize-none rounded bg-bg border border-border px-2 py-1.5 text-xs text-text placeholder:text-muted focus:outline-none focus:border-accent disabled:opacity-50"
        />
      </div>

      <p className="text-xs text-muted">
        Uses OpenRouter, {hasKey ? <span className="text-success">key set</span> : <span className="text-danger">no key — set one in Text Model</span>}.
      </p>

      <button
        onClick={onSave}
        disabled={saving || !model.trim()}
        className="mt-1 rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {saving ? "Saving..." : savedNotice ? "Saved" : "Save"}
      </button>
    </div>
  );
}
