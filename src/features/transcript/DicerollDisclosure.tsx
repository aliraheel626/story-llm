import type { Diceroll } from "../../shared/types";

export function DicerollDisclosure({ diceroll }: { diceroll: Diceroll }) {
  const source = diceroll.chance_source
    ? { default: "Default", narrator: "Narrator", attributes: "Attributes" }[diceroll.chance_source]
    : "Unspecified";
  const factors = diceroll.factors ?? [];
  return (
    <details className="text-[11px] text-muted">
      <summary className="cursor-pointer text-left hover:text-text">
        Diceroll: {diceroll.reason ? `${diceroll.reason} · ` : ""}{source} · {diceroll.chance_percent}% chance · needed {diceroll.needed}+ · rolled {diceroll.roll} · {diceroll.outcome}
        {factors.length > 0 && ` · ${factors.map((factor) => `${factor.entity_name}: ${factor.attribute_name}`).join(" vs ")}`}
      </summary>
      <div className="mt-1.5 rounded border border-border bg-bg px-2.5 py-2 leading-5">
        {diceroll.reason && <p className="text-text">Reason: {diceroll.reason}</p>}
        <p>Chance source: {source}. Chance: {diceroll.chance_percent}%. Needed: {diceroll.needed} or higher. Actual roll: {diceroll.roll}. Outcome: {diceroll.outcome}. Seed: {diceroll.seed}.</p>
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
