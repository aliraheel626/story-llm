import { AccordionPanel } from "../../shared/ui/AccordionPanel";
import { StoriesPanel } from "../stories/StoriesPanel";
import { CharactersPanel } from "../characters/CharactersPanel";
import { NarratorToolsPanel } from "../narratorTools/NarratorToolsPanel";
import { ContextPanel } from "../context/ContextPanel";
import { InjectionPanel } from "../context/InjectionPanel";
import { TextModelPanel } from "../settings/TextModelPanel";
import { ImageModelPanel } from "../settings/ImageModelPanel";
import { PlaceholderPanel } from "./PlaceholderPanel";
import { useAppShellStore } from "../../app/store";

const PANEL_LABELS: Record<string, string> = {
  stories: "Stories",
  characters: "Characters",
  narratorTools: "Narrator Tools",
  context: "Context",
  injection: "Injection",
  textModel: "Text Model",
  imageModel: "Image Model",
  features: "Features",
};

export function Sidebar() {
  const openPanels = useAppShellStore((s) => s.openPanels);
  const togglePanel = useAppShellStore((s) => s.togglePanel);
  const sidebarOpen = useAppShellStore((s) => s.sidebarOpen);
  const toggleSidebar = useAppShellStore((s) => s.toggleSidebar);

  if (!sidebarOpen) {
    return (
      <aside className="flex h-full w-12 shrink-0 flex-col items-center border-r border-border bg-surface py-3 transition-[width] duration-150 motion-reduce:transition-none">
        <button
          type="button"
          aria-label="Open sidebar"
          aria-expanded={false}
          title="Open sidebar (Ctrl+B)"
          onClick={toggleSidebar}
          className="rounded p-2 text-muted hover:bg-bg hover:text-text"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
            <path d="M4 6h16M4 12h16M4 18h16" />
          </svg>
        </button>
      </aside>
    );
  }

  return (
    <aside className="flex h-full w-64 shrink-0 flex-col overflow-hidden border-r border-border bg-surface transition-[width] duration-150 motion-reduce:transition-none">
      <div className="h-full w-64 shrink-0 overflow-y-auto">
        <div className="flex items-center justify-between border-b border-border px-3 py-3">
          <span className="font-prose text-sm tracking-wide text-text">story-llm</span>
          <button
            type="button"
            aria-label="Close sidebar"
            aria-expanded={true}
            title="Close sidebar (Ctrl+B)"
            onClick={toggleSidebar}
            className="rounded p-1 text-muted hover:bg-bg hover:text-text"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
              <path d="M3 3l10 10M13 3L3 13" />
            </svg>
          </button>
        </div>

        <AccordionPanel title={PANEL_LABELS.stories} open={openPanels.stories} onToggle={() => togglePanel("stories")}>
          <StoriesPanel />
        </AccordionPanel>

        <AccordionPanel title={PANEL_LABELS.characters} open={openPanels.characters} onToggle={() => togglePanel("characters")}>
          <CharactersPanel />
        </AccordionPanel>

        <AccordionPanel title={PANEL_LABELS.narratorTools} open={openPanels.narratorTools} onToggle={() => togglePanel("narratorTools")}>
          <NarratorToolsPanel />
        </AccordionPanel>

        <AccordionPanel title={PANEL_LABELS.context} open={openPanels.context} onToggle={() => togglePanel("context")}>
          <ContextPanel />
        </AccordionPanel>

        <AccordionPanel title={PANEL_LABELS.injection} open={openPanels.injection} onToggle={() => togglePanel("injection")}>
          <InjectionPanel />
        </AccordionPanel>

        <AccordionPanel title={PANEL_LABELS.textModel} open={openPanels.textModel} onToggle={() => togglePanel("textModel")}>
          <TextModelPanel />
        </AccordionPanel>

        <AccordionPanel title={PANEL_LABELS.imageModel} open={openPanels.imageModel} onToggle={() => togglePanel("imageModel")}>
          <ImageModelPanel />
        </AccordionPanel>

        <AccordionPanel title={PANEL_LABELS.features} open={openPanels.features} onToggle={() => togglePanel("features")}>
          <PlaceholderPanel label="Features" />
        </AccordionPanel>
      </div>
    </aside>
  );
}
