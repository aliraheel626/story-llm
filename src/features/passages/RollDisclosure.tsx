import { useState } from "react";
import type { EntityAttributeValue, RollDetail } from "../../shared/types";
import { useStoryStore } from "./store";

function attrLine(attrs: EntityAttributeValue[]): string {
  return attrs.map((a) => `${a.canonical_name} ${Math.round(a.value)}/${Math.round(a.max)}`).join(", ");
}

/** Roll-high-succeeds: mirrors src-tauri/src/mechanics/resolve.rs::needed_roll. */
function neededRoll(pSuccess: number): number {
  return Math.round(100 - pSuccess * 100);
}

function matchupLabel(summary: RollDetail): string {
  const actor = summary.actor_attribute_name ? `${summary.actor_attribute_name} (${summary.actor_name})` : summary.actor_name;
  if (!summary.target_name) return actor;
  const target = summary.target_attribute_name ? `${summary.target_attribute_name} (${summary.target_name})` : summary.target_name;
  return `${actor} vs ${target}`;
}

/** `summary` is the cheap, always-available version (names, no attribute
 * snapshots) from the branch-wide bulk load; `detail` (with snapshots) is
 * fetched lazily only once the user expands. */
export function RollDisclosure({ passageId, summary }: { passageId: string; summary: RollDetail }) {
  const [open, setOpen] = useState(false);
  const detail = useStoryStore((s) => s.rollDetailByPassage[passageId]);
  const loadRollDetail = useStoryStore((s) => s.loadRollDetail);

  const toggle = () => {
    if (!open && !detail) loadRollDetail(passageId);
    setOpen((v) => !v);
  };

  const { roll } = summary;
  const pct = Math.round(roll.p_success * 100);
  const needed = neededRoll(roll.p_success);

  return (
    <div className="text-[11px] text-muted">
      <button onClick={toggle} className="flex items-center gap-1 hover:text-text">
        <span>🎲</span>
        <span>
          {matchupLabel(summary)} — {pct}% chance → rolled {roll.roll}, needed {needed}+ → {roll.outcome} ({roll.degree})
        </span>
        <span className="text-muted">{open ? "▲" : "▼"}</span>
      </button>

      {open && (
        <div className="mt-1.5 rounded border border-border bg-bg px-2.5 py-2 leading-5">
          {!detail ? (
            <span>Loading...</span>
          ) : (
            <div className="flex flex-col gap-1">
              <div>
                <span className="text-text">{detail.actor_name}</span>
                {detail.actor_attribute_name && (
                  <>
                    {" "}
                    {detail.actor_attribute_name} {Math.round(roll.actor_value ?? 0)}
                  </>
                )}
                {detail.target_name && (
                  <>
                    {" "}
                    vs <span className="text-text">{detail.target_name}</span>
                    {detail.target_attribute_name && (
                      <>
                        {" "}
                        {detail.target_attribute_name} {Math.round(roll.target_value ?? 0)}
                      </>
                    )}
                  </>
                )}
              </div>
              <div>
                {pct}% chance → needed {needed}+, rolled {roll.roll} → {roll.outcome} ({roll.degree}). Seed {roll.seed}.
              </div>
              {detail.actor_attributes.length > 0 && (
                <div>
                  {detail.actor_name}: {attrLine(detail.actor_attributes)}
                </div>
              )}
              {detail.target_name && detail.target_attributes.length > 0 && (
                <div>
                  {detail.target_name}: {attrLine(detail.target_attributes)}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
