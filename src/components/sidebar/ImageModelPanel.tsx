import { useEffect, useState } from "react";
import { useImageModelStore } from "../../store/imageModelStore";
import { useTextModelStore } from "../../store/textModelStore";

const STYLE_PRESETS: { label: string; value: string }[] = [
  { label: "Painterly", value: "Digital painting, atmospheric scene illustration." },
  { label: "Anime", value: "Anime style illustration, vibrant colors, clean linework." },
  { label: "Realistic", value: "Photorealistic, cinematic lighting, high detail." },
  { label: "Comic", value: "Comic book art style, bold inked linework, dynamic shading." },
];

export function ImageModelPanel() {
  const settings = useImageModelStore((s) => s.settings);
  const loading = useImageModelStore((s) => s.loading);
  const saving = useImageModelStore((s) => s.saving);
  const load = useImageModelStore((s) => s.load);
  const save = useImageModelStore((s) => s.save);
  const textSettings = useTextModelStore((s) => s.settings);

  const [model, setModel] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [style, setStyle] = useState("");
  const [savedNotice, setSavedNotice] = useState(false);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    if (settings) {
      setModel(settings.model);
      setEnabled(settings.enabled);
      setStyle(settings.style);
    }
  }, [settings]);

  const onSave = async () => {
    try {
      await save(model.trim(), enabled, style.trim());
      setSavedNotice(true);
      setTimeout(() => setSavedNotice(false), 2000);
    } catch (e) {
      console.error(e);
    }
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

      <div>
        <label className="block text-xs text-muted mb-1">Model</label>
        <input
          value={model}
          onChange={(e) => setModel(e.target.value)}
          placeholder="google/gemini-3.1-flash-image-preview"
          disabled={!enabled}
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent disabled:opacity-50"
        />
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
