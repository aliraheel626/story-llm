"""Small tauri-pilot driver for the C7 checks finished after Codex stopped (10 and 11)."""
import json
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
PIPE = "\\\\.\\pipe\\tauri-pilot-com.story-llm.app"


def pilot(*arguments, stdin=None):
    done = subprocess.run(["tauri-pilot", "--socket", PIPE, "--json", *arguments],
                          capture_output=True, text=True, encoding="utf-8", input=stdin)
    if done.returncode:
        raise RuntimeError(done.stdout + done.stderr)
    return json.loads(done.stdout) if done.stdout.strip() else None


def js(script):
    """Evaluates a script passed on stdin, avoiding Windows argument quoting."""
    return pilot("eval", "-", stdin=script)


if __name__ == "__main__":
    print(json.dumps(pilot(*sys.argv[1:]), ensure_ascii=False, indent=1)[:6000])
