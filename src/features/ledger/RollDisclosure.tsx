import type { Roll } from "../../shared/types";

/** Roll-high-succeeds, with an integer draw from 0 through 99. */
export function RollDisclosure({ roll }: { roll: Roll }) {
  const needed = 100 - roll.chance_percent;
  const source = roll.chance_source
    ? { default: "Default", narrator: "Narrator", attributes: "Attributes" }[roll.chance_source]
    : "Unspecified";
  const factors = roll.factors ?? [];
  return (
    <details className="text-[11px] text-muted">
      <summary className="cursor-pointer text-left hover:text-text">
        Roll: {roll.reason ? `${roll.reason} · ` : ""}{source} · {roll.chance_percent}% chance · needed {needed}+ · rolled {roll.roll} · {roll.outcome}
        {factors.length > 0 && ` · ${factors.map((factor) => `${factor.entity_name}: ${factor.attribute_name}`).join(" vs ")}`}
      </summary>
      <div className="mt-1.5 rounded border border-border bg-bg px-2.5 py-2 leading-5">
        {roll.reason && <p className="text-text">Reason: {roll.reason}</p>}
        <p>Chance source: {source}. Chance: {roll.chance_percent}%. Needed: {needed} or higher (0-99). Actual roll: {roll.roll}. Outcome: {roll.outcome}. Seed: {roll.seed}.</p>
        {factors.length > 0 ? (
          <>
            <p>Values are normalized to their ranges. The first factor acts; the second opposes it, or a neutral midpoint is used when absent.</p>
            <ul className="list-disc pl-4">
              {factors.map((factor, index) => (
                <li key={`${factor.entity_id}:${factor.attribute_id}:${index}`}>
                  {factor.entity_name}: {factor.attribute_name} {factor.value} (range {factor.min}-{factor.max})
                </li>
              ))}
            </ul>
          </>
        ) : <p>No attribute factors.</p>}
      </div>
    </details>
  );
}
