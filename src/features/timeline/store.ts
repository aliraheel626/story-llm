import { create } from "zustand";
import type { NarrationDonePayload, NarrationToolActivityPayload, NarrationVariant, RollDetail, StoryImage, SwipeDonePayload, TimelineEntry } from "../../shared/types";
import { foldVisibleTimeline } from "../../shared/types";
import { useAppStore } from "../../app/store";
import { useCharacterStore } from "../characters/store";
import { timelineApi } from "./api";

type ToolActivity = { label: string; phase: "started" | "finished" };
interface StreamingState { streamId: string; branchId: string; text: string; thoughts: string; mode: "append" | "replace"; targetEntryId?: string; toolActivity?: ToolActivity | null; toolLog: ToolActivity[] }
/** The last finished turn's thinking and tool calls, kept so the composer's
 *  activity panel still has something to show once streaming ends. Reasoning
 *  and tool activity aren't persisted server-side, so this is session-only. */
interface TurnActivity { thoughts: string; tools: ToolActivity[] }
interface StoryState {
  entriesByBranch: Record<string, TimelineEntry[]>; timelineLoading: boolean;
  /** Hidden events per branch, kept so a turn's committed tool calls can be
   *  reconstructed under the narration they produced. Not rendered directly. */
  hiddenByBranch: Record<string, TimelineEntry[]>;
  streamingByBranch: Record<string, StreamingState>; turnError: string | null;
  turnActivityByBranch: Record<string, TurnActivity>;
  variantsByEntry: Record<string, NarrationVariant[]>;
  imagesByEntry: Record<string, StoryImage[]>; imagePendingFor: string[]; imageError: string | null;
  rollByEntry: Record<string, RollDetail>; rollDetailByEntry: Record<string, RollDetail>;
  loadTimeline: (branchId: string) => Promise<void>;
  submitStoryText: (branchId: string, content: string) => Promise<void>;
  submitTurn: (branchId: string, mode: "do" | "say", content: string) => Promise<void>;
  submitGuide: (branchId: string, note: string) => Promise<void>; continueScene: (branchId: string) => Promise<void>;
  retryNarration: (branchId: string, entryId: string) => Promise<void>; generateVariant: (branchId: string, entryId: string) => Promise<void>;
  eraseLastExchange: (branchId: string) => Promise<void>; editEntry: (branchId: string, entryId: string, content: string) => Promise<void>;
  selectVariant: (branchId: string, entryId: string, variantEntryId: string) => Promise<void>; loadVariantsForEntry: (entryId: string) => Promise<void>;
  loadImagesForBranch: (branchId: string) => Promise<void>; generateImageForEntry: (entryId: string, hint?: string) => Promise<void>;
  loadRollsForBranch: (branchId: string) => Promise<void>; loadRollDetail: (entryId: string) => Promise<void>;
  _appendDelta: (streamId: string, text: string) => void; _appendThoughts: (streamId: string, text: string) => void;
  _toolActivity: (payload: NarrationToolActivityPayload) => void;
  _finalize: (payload: NarrationDonePayload) => void; _swipeDone: (payload: SwipeDonePayload) => void; _fail: (streamId: string, message: string) => void;
  _imagePending: (entryId: string) => void; _imageGenerated: (image: StoryImage) => void; _imageFailed: (entryId: string) => void;
}

const replaceEntry = (entries: TimelineEntry[], id: string, next: TimelineEntry) => entries.map((entry) => entry.id === id ? next : entry);
const findStream = (streams: Record<string, StreamingState>, id: string) => Object.entries(streams).find(([, stream]) => stream.streamId === id);
const withoutStream = (streams: Record<string, StreamingState>, branchId: string) => { const next = { ...streams }; delete next[branchId]; return next; };
const removeOne = (items: string[], value: string) => { const i = items.indexOf(value); return i < 0 ? items : items.slice(0, i).concat(items.slice(i + 1)); };
const newStream = (streamId: string, branchId: string, mode: "append" | "replace", targetEntryId?: string): StreamingState =>
  ({ streamId, branchId, text: "", thoughts: "", mode, targetEntryId, toolLog: [] });
const rememberActivity = (byBranch: Record<string, TurnActivity>, branchId: string, stream: StreamingState) =>
  ({ ...byBranch, [branchId]: { thoughts: stream.thoughts, tools: stream.toolLog } });
const closeOpenTool = (log: ToolActivity[]): ToolActivity[] => {
  const open = log.map((tool) => tool.phase).lastIndexOf("started");
  return open < 0 ? log : log.map((tool, i) => (i === open ? { ...tool, phase: "finished" as const } : tool));
};

export const useStoryStore = create<StoryState>((set, get) => ({
  entriesByBranch: {}, timelineLoading: false, streamingByBranch: {}, turnError: null, hiddenByBranch: {},
  turnActivityByBranch: {},
  variantsByEntry: {}, imagesByEntry: {}, imagePendingFor: [], imageError: null, rollByEntry: {}, rollDetailByEntry: {},
  loadTimeline: async (branchId) => {
    set({ timelineLoading: true });
    try { const raw = await timelineApi.list(branchId); set((s) => ({ entriesByBranch: { ...s.entriesByBranch, [branchId]: foldVisibleTimeline(raw) }, hiddenByBranch: { ...s.hiddenByBranch, [branchId]: raw.filter((entry) => entry.visibility === "hidden") }, timelineLoading: false })); }
    catch (e) { console.error("failed to load timeline", e); set({ timelineLoading: false }); }
  },
  submitStoryText: async (branchId, content) => {
    set({ turnError: null }); const result = await timelineApi.submitStory(branchId, content);
    set((s) => ({ entriesByBranch: { ...s.entriesByBranch, [branchId]: [...(s.entriesByBranch[branchId] ?? []), result.entry] }, streamingByBranch: { ...s.streamingByBranch, [branchId]: newStream(result.stream_id, branchId, "append") } }));
  },
  submitTurn: async (branchId, mode, content) => {
    set({ turnError: null }); const result = await timelineApi.submitTurn(branchId, mode, content);
    set((s) => ({ entriesByBranch: { ...s.entriesByBranch, [branchId]: [...(s.entriesByBranch[branchId] ?? []), result.entry] }, streamingByBranch: { ...s.streamingByBranch, [branchId]: newStream(result.stream_id, branchId, "append") } }));
  },
  submitGuide: async (branchId, note) => { set({ turnError: null }); const streamId = await timelineApi.submitGuide(branchId, note); set((s) => ({ streamingByBranch: { ...s.streamingByBranch, [branchId]: newStream(streamId, branchId, "append") } })); },
  continueScene: async (branchId) => { set({ turnError: null }); const streamId = await timelineApi.continueScene(branchId); set((s) => ({ streamingByBranch: { ...s.streamingByBranch, [branchId]: newStream(streamId, branchId, "append") } })); },
  retryNarration: async (branchId, entryId) => { set({ turnError: null }); const result = await timelineApi.retry(branchId, entryId); set((s) => ({ streamingByBranch: { ...s.streamingByBranch, [branchId]: newStream(result.stream_id, branchId, "replace", result.entry_id) } })); },
  generateVariant: async (branchId, entryId) => { set({ turnError: null }); const streamId = await timelineApi.generateVariant(branchId, entryId); set((s) => ({ streamingByBranch: { ...s.streamingByBranch, [branchId]: newStream(streamId, branchId, "replace", entryId) } })); },
  eraseLastExchange: async (branchId) => {
    const ids = await timelineApi.eraseLastExchange(branchId);
    if (!ids.length) return;
    set((s) => {
      const images = { ...s.imagesByEntry };
      const variants = { ...s.variantsByEntry };
      const rolls = { ...s.rollByEntry };
      const rollDetails = { ...s.rollDetailByEntry };
      ids.forEach((id) => {
        delete images[id]; delete variants[id]; delete rolls[id]; delete rollDetails[id];
      });
      return {
        entriesByBranch: { ...s.entriesByBranch, [branchId]: (s.entriesByBranch[branchId] ?? []).filter((entry) => !ids.includes(entry.id)) },
        imagesByEntry: images,
        variantsByEntry: variants,
        rollByEntry: rolls,
        rollDetailByEntry: rollDetails,
        imagePendingFor: s.imagePendingFor.filter((id) => !ids.includes(id)),
      };
    });
    const storyId = useAppStore.getState().activeStoryId;
    if (storyId) await useCharacterStore.getState().loadCharacters(storyId, branchId);
  },
  editEntry: async (branchId, entryId, content) => { const entry = await timelineApi.edit(entryId, content); set((s) => ({ entriesByBranch: { ...s.entriesByBranch, [branchId]: replaceEntry(s.entriesByBranch[branchId] ?? [], entryId, entry) }, imagesByEntry: { ...s.imagesByEntry, [entryId]: [] } })); },
  selectVariant: async (branchId, entryId, variantEntryId) => { const entry = await timelineApi.selectVariant(entryId, variantEntryId); set((s) => ({ entriesByBranch: { ...s.entriesByBranch, [branchId]: replaceEntry(s.entriesByBranch[branchId] ?? [], entryId, entry) }, variantsByEntry: { ...s.variantsByEntry, [entryId]: (s.variantsByEntry[entryId] ?? []).map((v) => ({ ...v, is_selected: v.id === variantEntryId })) }, imagesByEntry: { ...s.imagesByEntry, [entryId]: [] } })); },
  loadVariantsForEntry: async (entryId) => { try { const variants = await timelineApi.listVariants(entryId); set((s) => ({ variantsByEntry: { ...s.variantsByEntry, [entryId]: variants } })); } catch (e) { console.error("failed to load variants", e); } },
  loadImagesForBranch: async (branchId) => { try { const list = await timelineApi.listImages(branchId); const grouped: Record<string, StoryImage[]> = {}; list.forEach((image) => (grouped[image.entry_id] ??= []).push(image)); set((s) => ({ imagesByEntry: { ...s.imagesByEntry, ...grouped } })); } catch (e) { console.error("failed to load images", e); } },
  generateImageForEntry: async (entryId, hint) => { set({ imageError: null }); get()._imagePending(entryId); try { get()._imageGenerated(await timelineApi.generateImage(entryId, hint)); } catch (e) { get()._imageFailed(entryId); set({ imageError: String(e) }); } },
  loadRollsForBranch: async (branchId) => { try { const list = await timelineApi.listRolls(branchId); const byEntry: Record<string, RollDetail> = {}; list.forEach((detail) => byEntry[detail.roll.entry_id] = detail); set((s) => ({ rollByEntry: { ...s.rollByEntry, ...byEntry } })); } catch (e) { console.error("failed to load rolls", e); } },
  loadRollDetail: async (entryId) => { try { const detail = await timelineApi.getRollDetail(entryId); if (detail) set((s) => ({ rollDetailByEntry: { ...s.rollDetailByEntry, [entryId]: detail } })); } catch (e) { console.error("failed to load roll detail", e); } },
  _appendDelta: (id, text) => { const found = findStream(get().streamingByBranch, id); if (!found) return; const [branchId, current] = found; set((s) => ({ streamingByBranch: { ...s.streamingByBranch, [branchId]: { ...current, text: current.text + text } } })); },
  _appendThoughts: (id, text) => { const found = findStream(get().streamingByBranch, id); if (!found) return; const [branchId, current] = found; set((s) => ({ streamingByBranch: { ...s.streamingByBranch, [branchId]: { ...current, thoughts: current.thoughts + text } } })); },
  _toolActivity: (payload) => {
    const found = findStream(get().streamingByBranch, payload.stream_id);
    if (!found) return;
    const [branchId, current] = found;
    // A tool call emits `started` then `finished`; keep them paired so the
    // panel lists each call once with its current state.
    let toolLog: ToolActivity[];
    if (payload.phase === "started") {
      toolLog = [...current.toolLog, { label: payload.label, phase: "started" }];
    } else {
      const closed = closeOpenTool(current.toolLog);
      toolLog = closed === current.toolLog ? [...current.toolLog, { label: payload.label, phase: "finished" }] : closed;
    }
    set((s) => ({ streamingByBranch: { ...s.streamingByBranch, [branchId]: { ...current, toolLog, toolActivity: { label: payload.label, phase: payload.phase } } } }));
  },
  _finalize: (payload) => { const found = findStream(get().streamingByBranch, payload.stream_id); if (!found) return; const [, current] = found; const branchId = payload.entry.branch_id; set((s) => ({ entriesByBranch: { ...s.entriesByBranch, [branchId]: current.mode === "replace" && current.targetEntryId ? replaceEntry(s.entriesByBranch[branchId] ?? [], current.targetEntryId, payload.entry) : [...(s.entriesByBranch[branchId] ?? []), payload.entry] }, streamingByBranch: withoutStream(s.streamingByBranch, branchId), turnActivityByBranch: rememberActivity(s.turnActivityByBranch, branchId, current), ...(current.mode === "replace" && current.targetEntryId ? { imagesByEntry: { ...s.imagesByEntry, [current.targetEntryId]: [] } } : {}) })); if (current.mode === "replace" && current.targetEntryId) get().loadVariantsForEntry(current.targetEntryId); get().loadRollsForBranch(branchId); },
  _swipeDone: (payload) => { const found = findStream(get().streamingByBranch, payload.stream_id); if (!found) return; const [, current] = found; const branchId = current.branchId; set((s) => ({ entriesByBranch: { ...s.entriesByBranch, [branchId]: replaceEntry(s.entriesByBranch[branchId] ?? [], payload.entry.id, payload.entry) }, variantsByEntry: { ...s.variantsByEntry, [payload.entry.id]: payload.variants }, imagesByEntry: { ...s.imagesByEntry, [payload.entry.id]: [] }, streamingByBranch: withoutStream(s.streamingByBranch, branchId), turnActivityByBranch: rememberActivity(s.turnActivityByBranch, branchId, current) })); },
  _fail: (id, message) => { const found = findStream(get().streamingByBranch, id); if (!found) return; const [branchId, current] = found; set((s) => ({ streamingByBranch: withoutStream(s.streamingByBranch, branchId), turnActivityByBranch: rememberActivity(s.turnActivityByBranch, branchId, current), turnError: message })); },
  _imagePending: (id) => set((s) => ({ imagePendingFor: s.imagePendingFor.includes(id) ? s.imagePendingFor : [...s.imagePendingFor, id] })),
  _imageGenerated: (image) => set((s) => ({ imagesByEntry: { ...s.imagesByEntry, [image.entry_id]: [...(s.imagesByEntry[image.entry_id] ?? []), image] }, imagePendingFor: removeOne(s.imagePendingFor, image.entry_id) })),
  _imageFailed: (id) => set((s) => ({ imagePendingFor: removeOne(s.imagePendingFor, id) })),
}));
