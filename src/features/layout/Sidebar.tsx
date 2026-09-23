import { AccordionPanel } from "../../shared/ui/AccordionPanel";
import { StoriesPanel } from "../stories/StoriesPanel";
import { CharactersPanel } from "../characters/CharactersPanel";
import { AttributesPanel } from "../dicerolls/AttributesPanel";
import { WritingStylePanel } from "../writingStyle/WritingStylePanel";
import { TextModelPanel } from "../settings/TextModelPanel";
import { ImageModelPanel } from "../settings/ImageModelPanel";
import { ContextInjectionPanel } from "../settings/ContextInjectionPanel";
import { LedgerRetentionPanel } from "../settings/LedgerRetentionPanel";
import { PlaceholderPanel } from "./PlaceholderPanel";
import { useAppShellStore } from "../../app/store";

const PANEL_LABELS: Record<string, string> = {
  stories: "Stories",
  characters: "Characters",
  writingStyle: "Writing Style",
  attributes: "Attributes",
  textModel: "Text Model",
  imageModel: "Image Model",
  contextInjection: "Context Injection",
  ledgerRetention: "Ledger Retention",
  features: "Features",
};

export function Sidebar() {
  const openPanels = useAppShellStore((s) => s.openPanels);
  const togglePanel = useAppShellStore((s) => s.togglePanel);

  return (
    <aside className="flex h-full w-64 shrink-0 flex-col overflow-y-auto border-r border-border bg-surface">
      <div className="px-3 py-3 border-b border-border">
        <span className="font-prose text-sm tracking-wide text-text">Dungeon</span>
      </div>

      <AccordionPanel title={PANEL_LABELS.stories} open={openPanels.stories} onToggle={() => togglePanel("stories")}>
        <StoriesPanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.characters} open={openPanels.characters} onToggle={() => togglePanel("characters")}>
        <CharactersPanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.writingStyle} open={openPanels.writingStyle} onToggle={() => togglePanel("writingStyle")}>
        <WritingStylePanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.attributes} open={openPanels.attributes} onToggle={() => togglePanel("attributes")}>
        <AttributesPanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.textModel} open={openPanels.textModel} onToggle={() => togglePanel("textModel")}>
        <TextModelPanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.imageModel} open={openPanels.imageModel} onToggle={() => togglePanel("imageModel")}>
        <ImageModelPanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.contextInjection} open={openPanels.contextInjection} onToggle={() => togglePanel("contextInjection")}>
        <ContextInjectionPanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.ledgerRetention} open={openPanels.ledgerRetention} onToggle={() => togglePanel("ledgerRetention")}>
        <LedgerRetentionPanel />
      </AccordionPanel>

      <AccordionPanel title={PANEL_LABELS.features} open={openPanels.features} onToggle={() => togglePanel("features")}>
        <PlaceholderPanel label="Features" />
      </AccordionPanel>
    </aside>
  );
}
