"""Classify staged numstat lines as production or tests, retaining the raw diff."""
import argparse
import json
import re
import subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[3]
evidence = Path(__file__).resolve().parent


def git(*arguments):
    return subprocess.run(["git", *arguments], cwd=root, capture_output=True, text=True, encoding="utf-8", errors="replace").stdout


def test_lines(path, text):
    lines = text.splitlines()
    if ".test." in path or path.startswith("tests/"):
        return set(range(1, len(lines) + 1))
    result = set()
    for index, line in enumerate(lines):
        if not re.match(r"\s*#\[cfg\(test\)\]", line):
            continue
        indent = len(line) - len(line.lstrip())
        end = index + 1
        while end < len(lines) and (lines[end].lstrip().startswith("#") or not lines[end].strip()):
            end += 1
        declaration = lines[end].strip() if end < len(lines) else ""
        if declaration.startswith(("use ", "pub use ", "pub(crate) use ")):
            while end < len(lines) and not lines[end].rstrip().endswith(";"):
                end += 1
        else:
            while end < len(lines) and lines[end] != " " * indent + "}":
                end += 1
        result.update(range(index + 1, min(end + 2, len(lines) + 1)))
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("step")
    args = parser.parse_args()
    raw = git("diff", "--cached", "--numstat", "--no-renames")
    diff = git("diff", "--cached", "--no-renames", "--unified=0")
    totals = {"production_added": 0, "production_removed": 0, "test_added": 0, "test_removed": 0}
    path = ""
    old_line = new_line = 0
    old_tests = new_tests = set()
    for line in diff.splitlines():
        if line.startswith("diff --git "):
            path = line.split(" b/", 1)[1]
            old_tests = test_lines(path, git("show", f"HEAD:{path}"))
            new_tests = test_lines(path, git("show", f":{path}"))
        elif line.startswith("@@"):
            match = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@", line)
            old_line, new_line = map(int, match.groups())
        elif line.startswith(("---", "+++")):
            continue
        elif line.startswith("-"):
            totals["test_removed" if old_line in old_tests else "production_removed"] += 1
            old_line += 1
        elif line.startswith("+"):
            totals["test_added" if new_line in new_tests else "production_added"] += 1
            new_line += 1
    totals["production_net"] = totals["production_added"] - totals["production_removed"]
    totals["test_net"] = totals["test_added"] - totals["test_removed"]
    entries = [line.split("\t", 2) for line in raw.splitlines() if not line.startswith("-\t")]
    assert sum(int(entry[0]) for entry in entries) == totals["production_added"] + totals["test_added"]
    assert sum(int(entry[1]) for entry in entries) == totals["production_removed"] + totals["test_removed"]
    totals["numstat"] = raw
    (evidence / f"{args.step}-numstat.txt").write_text(raw, encoding="utf-8")
    (evidence / f"{args.step}-diff.patch").write_text(diff, encoding="utf-8")
    (evidence / f"{args.step}-audit.json").write_text(json.dumps(totals, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(totals, indent=2))
