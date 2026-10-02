"""Byte-exact contract comparison with hashes as audit evidence."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("left")
parser.add_argument("right")
parser.add_argument("output")
args = parser.parse_args()
root = Path(__file__).resolve().parent
left, right = root / args.left, root / args.right
before, after = left.read_bytes(), right.read_bytes()
result = {
    "left": str(left),
    "right": str(right),
    "byte_identical": before == after,
    "before_sha256": hashlib.sha256(before).hexdigest(),
    "after_sha256": hashlib.sha256(after).hexdigest(),
    "before_bytes": len(before),
    "after_bytes": len(after),
}
(root / args.output).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
print(json.dumps(result, indent=2))
raise SystemExit(0 if before == after else 1)
