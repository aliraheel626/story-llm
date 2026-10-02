"""Scores bk-*.json: for each expected record, on time (that turn), late (next two turns) or missed."""
import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")
DIR = Path(sys.argv[1])


def calls(turn, tool):
    return [c for c in turn["tools"] if c["tool"] == tool and c["ok"]]


def is_hedda(name):
    return "hedda" in str(name or "").lower()


def is_you(name):
    return str(name or "").strip().lower() == "you"


def hedda_you(c):
    a = c["args"]
    return {is_hedda(a.get("from")), is_you(a.get("from"))} | {is_hedda(a.get("to")), is_you(a.get("to"))} >= {True} and \
        (is_hedda(a.get("from")) or is_hedda(a.get("to"))) and (is_you(a.get("from")) or is_you(a.get("to")))


CHECKS = {
    1: lambda t: any(is_hedda(c["args"].get("name")) for c in calls(t, "save_character")),
    2: lambda t: any((c.get("result") or {}).get("created") and not is_hedda(c["args"].get("name")) and not is_you(c["args"].get("name"))
                     for c in calls(t, "save_character")),
    3: lambda t: any(c["args"].get("known_as") for c in calls(t, "save_character")),
    4: lambda t: bool(calls(t, "roll_check")),
    5: lambda t: not t["tools"],
    6: lambda t: any(hedda_you(c) for c in calls(t, "save_relationship")),
    7: lambda t: any(hedda_you(c) and c["args"].get("stats") for c in calls(t, "save_relationship")),
    8: lambda t: any(is_hedda(c["args"].get("name")) and c["args"].get("stats") for c in calls(t, "save_character")),
    9: lambda t: any(is_hedda(c["args"].get("name")) and (c["args"].get("location") or c["args"].get("outfit")) for c in calls(t, "save_character")),
    10: lambda t: any(hedda_you(c) for c in calls(t, "save_relationship")),
}
LABELS = {1: "Hedda created", 2: "old man (narrator-named)", 3: "hooded figure, known_as", 4: "roll for the lock",
          5: "nothing on a routine action", 6: "Hedda/You relationship", 7: "relationship stats", 8: "Hedda injury stats",
          9: "Hedda location/outfit", 10: "relationship after the theft"}


def score(run):
    turns = run["turns"]
    out = {}
    for index, check in CHECKS.items():
        if check(turns[index - 1]):
            out[index] = "on time"
        elif index != 5 and any(check(t) for t in turns[index:index + 2]):
            out[index] = "late"
        else:
            out[index] = "missed" if index != 5 else "extra calls"
    return out


rows = {}
summary = {}
for path in sorted(DIR.glob("bk-*.json")):
    run = json.loads(path.read_text(encoding="utf-8"))
    arm = run["arm"]
    result = score(run)
    rows.setdefault(arm, []).append(result)
    total_calls = sum(len(t["tools"]) for t in run["turns"])
    rolls = sum(len(calls(t, "roll_check")) for t in run["turns"])
    cost = sum(u["cost_usd"] or 0 for u in run["usage"])
    narr = sum(u["calls"] for u in run["usage"] if u["kind"] == "narration")
    s = summary.setdefault(arm, {"on time": 0, "late": 0, "missed": 0, "calls": 0, "rolls": 0, "cost": 0.0, "narration_calls": 0})
    for v in result.values():
        s[{"extra calls": "missed"}.get(v, v)] += 1
    s["calls"] += total_calls
    s["rolls"] += rolls
    s["cost"] += cost
    s["narration_calls"] += narr

arms = sorted(rows)
print("| # | Expected | " + " | ".join(arms) + " |")
print("|---|---|" + "---|" * len(arms))
for index in CHECKS:
    cells = []
    for arm in arms:
        values = [r[index] for r in rows[arm]]
        cells.append(", ".join(values))
    print(f"| {index} | {LABELS[index]} | " + " | ".join(cells) + " |")
print()
for arm in arms:
    print(arm, json.dumps({k: (round(v, 4) if isinstance(v, float) else v) for k, v in summary[arm].items()}))
