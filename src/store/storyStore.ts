import { create } from "zustand";
import { commands } from "../lib/commands";
import type { NarrationDonePayload, Passage, PassageVariant, RollDetail, StoryImage, SwipeDonePayload } from "../lib/types";

interface StreamingState {
  streamId: string;
  branchId: string;
  text: string;
  thoughts: string;
  /** "append" adds a new trailing passage on completion (turn/guide/continue).
   *  "replace" shows the live buffer inside an existing passage's slot
   *  (retry/swipe) until that passage is replaced or updated in place. */
  mode: "append" | "replace";
  targetPassageId?: string;
}

interface StoryState {
  passagesByBranch: Record<string, Passage[]>;
  passagesLoading: boolean;
  streaming: StreamingState | null;
  turnError: string | null;

  variantsByPassage: Record<string, PassageVariant[]>;

  imagesByPassage: Record<string, StoryImage[]>;
  /** Passages with an image in flight — the player's own "See" and the
   *  narrator's own decision both land here, so both show a placeholder. */
  imagePendingFor: string[];
  imageError: string | null;

  /** Cheap summary (names, no attribute snapshots) for every roll in the branch. */
  rollByPassage: Record<string, RollDetail>;
  /** Full detail (attribute snapshots included), lazy-loaded on expand. */
  rollDetailByPassage: Record<string, RollDetail>;

  loadPassages: (branchId: string) => Promise<void>;
  submitStoryText: (branchId: string, content: string) => Promise<void>;
  submitTurn: (branchId: string, mode: "do" | "say", content: string) => Promise<void>;
  submitGuide: (branchId: string, note: string) => Promise<void>;
  continueScene: (branchId: string) => Promise<void>;
  retryPassage: (branchId: string, passageId: string) => Promise<void>;
  swipePassage: (branchId: string, passageId: string) => Promise<void>;
  eraseLastExchange: (branchId: string) => Promise<void>;
  editPassage: (branchId: string, passageId: string, content: string) => Promise<void>;
  switchVariant: (branchId: string, passageId: string, variantId: string) => Promise<void>;
  loadVariantsForPassage: (passageId: string) => Promise<void>;

  loadImagesForBranch: (branchId: string) => Promise<void>;
  generateImageForPassage: (passageId: string, promptHint?: string) => Promise<void>;

  loadRollsForBranch: (branchId: string) => Promise<void>;
  loadRollDetail: (passageId: string) => Promise<void>;

  _appendDelta: (streamId: string, text: string) => void;
  _appendThoughts: (streamId: string, text: string) => void;
  _finalize: (payload: NarrationDonePayload) => void;
  _swipeDone: (payload: SwipeDonePayload) => void;
  _fail: (streamId: string, message: string) => void;
  _imagePending: (passageId: string) => void;
  _imageGenerated: (image: StoryImage) => void;
  _imageFailed: (passageId: string) => void;
}

function replacePassage(passages: Passage[], id: string, next: Passage): Passage[] {
  const idx = passages.findIndex((p) => p.id === id);
  if (idx === -1) return passages;
  const copy = passages.slice();
  copy[idx] = next;
  return copy;
}

export const useStoryStore = create<StoryState>((set, get) => ({
  passagesByBranch: {},
  passagesLoading: false,
  streaming: null,
  turnError: null,

  variantsByPassage: {},

  imagesByPassage: {},
  imagePendingFor: [],
  imageError: null,

  rollByPassage: {},
  rollDetailByPassage: {},

  loadPassages: async (branchId: string) => {
    set({ passagesLoading: true });
    try {
      const passages = await commands.listPassages(branchId);
      set((s) => ({ passagesByBranch: { ...s.passagesByBranch, [branchId]: passages }, passagesLoading: false }));
    } catch (e) {
      console.error("failed to load passages", e);
      set({ passagesLoading: false });
    }
  },

  submitStoryText: async (branchId: string, content: string) => {
    set({ turnError: null });
    const result = await commands.submitStory(branchId, content);
    set((s) => ({
      passagesByBranch: {
        ...s.passagesByBranch,
        [branchId]: [...(s.passagesByBranch[branchId] ?? []), result.passage],
      },
      streaming: { streamId: result.stream_id, branchId, text: "", thoughts: "", mode: "append" },
    }));
  },

  submitTurn: async (branchId: string, mode: "do" | "say", content: string) => {
    set({ turnError: null });
    const result = await commands.submitTurn(branchId, mode, content);
    set((s) => ({
      passagesByBranch: {
        ...s.passagesByBranch,
        [branchId]: [...(s.passagesByBranch[branchId] ?? []), result.passage],
      },
      streaming: { streamId: result.stream_id, branchId, text: "", thoughts: "", mode: "append" },
    }));
  },

  submitGuide: async (branchId: string, note: string) => {
    set({ turnError: null });
    const streamId = await commands.submitGuide(branchId, note);
    set({ streaming: { streamId, branchId, text: "", thoughts: "", mode: "append" } });
  },

  continueScene: async (branchId: string) => {
    set({ turnError: null });
    const streamId = await commands.continueScene(branchId);
    set({ streaming: { streamId, branchId, text: "", thoughts: "", mode: "append" } });
  },

  retryPassage: async (branchId: string, passageId: string) => {
    set({ turnError: null });
    const result = await commands.retryPassage(branchId, passageId);
    set({
      streaming: {
        streamId: result.stream_id,
        branchId,
        text: "",
        thoughts: "",
        mode: "replace",
        targetPassageId: result.removed_passage_id,
      },
    });
  },

  swipePassage: async (branchId: string, passageId: string) => {
    set({ turnError: null });
    const streamId = await commands.swipePassage(branchId, passageId);
    set({
      streaming: { streamId, branchId, text: "", thoughts: "", mode: "replace", targetPassageId: passageId },
    });
  },

  eraseLastExchange: async (branchId: string) => {
    const removedIds = await commands.eraseLastExchange(branchId);
    if (removedIds.length === 0) return;
    set((s) => ({
      passagesByBranch: {
        ...s.passagesByBranch,
        [branchId]: (s.passagesByBranch[branchId] ?? []).filter((p) => !removedIds.includes(p.id)),
      },
    }));
  },

  editPassage: async (branchId: string, passageId: string, content: string) => {
    const passage = await commands.editPassage(passageId, content);
    set((s) => ({
      passagesByBranch: { ...s.passagesByBranch, [branchId]: replacePassage(s.passagesByBranch[branchId] ?? [], passageId, passage) },
    }));
  },

  switchVariant: async (branchId: string, passageId: string, variantId: string) => {
    const passage = await commands.switchVariant(passageId, variantId);
    set((s) => ({
      passagesByBranch: { ...s.passagesByBranch, [branchId]: replacePassage(s.passagesByBranch[branchId] ?? [], passageId, passage) },
      variantsByPassage: {
        ...s.variantsByPassage,
        [passageId]: (s.variantsByPassage[passageId] ?? []).map((v) => ({ ...v, is_selected: v.id === variantId })),
      },
    }));
  },

  loadVariantsForPassage: async (passageId: string) => {
    try {
      const variants = await commands.listVariants(passageId);
      if (variants.length > 0) {
        set((s) => ({ variantsByPassage: { ...s.variantsByPassage, [passageId]: variants } }));
      }
    } catch (e) {
      console.error("failed to load variants", e);
    }
  },

  loadImagesForBranch: async (branchId: string) => {
    try {
      const images = await commands.listImagesForBranch(branchId);
      const grouped: Record<string, StoryImage[]> = {};
      for (const image of images) {
        (grouped[image.passage_id] ??= []).push(image);
      }
      set((s) => ({ imagesByPassage: { ...s.imagesByPassage, ...grouped } }));
    } catch (e) {
      console.error("failed to load images for branch", e);
    }
  },

  generateImageForPassage: async (passageId: string, promptHint?: string) => {
    set({ imageError: null });
    get()._imagePending(passageId);
    try {
      const image = await commands.generateSceneImage(passageId, promptHint);
      get()._imageGenerated(image);
    } catch (e) {
      get()._imageFailed(passageId);
      set({ imageError: String(e) });
    }
  },

  loadRollsForBranch: async (branchId: string) => {
    try {
      const rolls = await commands.listRollsForBranch(branchId);
      const byPassage: Record<string, RollDetail> = {};
      for (const summary of rolls) byPassage[summary.roll.passage_id] = summary;
      set((s) => ({ rollByPassage: { ...s.rollByPassage, ...byPassage } }));
    } catch (e) {
      console.error("failed to load rolls for branch", e);
    }
  },

  loadRollDetail: async (passageId: string) => {
    try {
      const detail = await commands.getRollDetail(passageId);
      if (detail) {
        set((s) => ({ rollDetailByPassage: { ...s.rollDetailByPassage, [passageId]: detail } }));
      }
    } catch (e) {
      console.error("failed to load roll detail", e);
    }
  },

  _appendDelta: (streamId, text) => {
    const current = get().streaming;
    if (!current || current.streamId !== streamId) return;
    set({ streaming: { ...current, text: current.text + text } });
  },

  _appendThoughts: (streamId, text) => {
    const current = get().streaming;
    if (!current || current.streamId !== streamId) return;
    set({ streaming: { ...current, thoughts: current.thoughts + text } });
  },

  _finalize: (payload) => {
    const current = get().streaming;
    if (!current || current.streamId !== payload.stream_id) return;
    const branchId = payload.passage.branch_id;
    set((s) => {
      const existing = s.passagesByBranch[branchId] ?? [];
      const next =
        current.mode === "replace" && current.targetPassageId
          ? existing.filter((p) => p.id !== current.targetPassageId).concat(payload.passage)
          : existing.concat(payload.passage);
      return { passagesByBranch: { ...s.passagesByBranch, [branchId]: next }, streaming: null };
    });
    // A Stage 1/2 roll may have landed for the new passage — pick it up.
    get().loadRollsForBranch(branchId);
  },

  _swipeDone: (payload) => {
    const current = get().streaming;
    if (!current || current.streamId !== payload.stream_id) return;
    const branchId = current.branchId;
    set((s) => ({
      passagesByBranch: {
        ...s.passagesByBranch,
        [branchId]: replacePassage(s.passagesByBranch[branchId] ?? [], payload.passage.id, payload.passage),
      },
      variantsByPassage: { ...s.variantsByPassage, [payload.passage.id]: payload.variants },
      streaming: null,
    }));
  },

  _fail: (streamId, message) => {
    const current = get().streaming;
    if (!current || current.streamId !== streamId) return;
    set({ streaming: null, turnError: message });
  },

  _imagePending: (passageId) => {
    set((s) => (s.imagePendingFor.includes(passageId) ? s : { imagePendingFor: [...s.imagePendingFor, passageId] }));
  },

  // Shared by the player's own "See" and the narrator's scene-image-generated
  // event, so an image lands the same way whoever asked for it.
  _imageGenerated: (image) => {
    set((s) => ({
      imagesByPassage: {
        ...s.imagesByPassage,
        [image.passage_id]: [...(s.imagesByPassage[image.passage_id] ?? []), image],
      },
      imagePendingFor: s.imagePendingFor.filter((id) => id !== image.passage_id),
    }));
  },

  _imageFailed: (passageId) => {
    set((s) => ({ imagePendingFor: s.imagePendingFor.filter((id) => id !== passageId) }));
  },
}));
