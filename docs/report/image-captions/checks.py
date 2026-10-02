import os
import subprocess
import sys
from pathlib import Path

evidence = Path(__file__).resolve().parent
workspace = evidence.parents[2]
label = sys.argv[1]
cwd = Path(sys.argv[2]) if len(sys.argv) > 2 else workspace
env = dict(os.environ, CARGO_TARGET_DIR=str(workspace / 'src-tauri' / 'target'))
commands = [
    'cargo test --manifest-path src-tauri/Cargo.toml',
    'cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets',
    'node --test src/features/story/store.test.mjs src/features/transcript/replacement.test.mjs src/features/usage/store.test.mjs',
    'npx tsc --noEmit',
]
for i, command in enumerate(commands, 1):
    result = subprocess.run(command, cwd=cwd, env=env, shell=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    text = result.stdout.decode('utf-8', errors='replace')
    (evidence / f'{label}-{i}.txt').write_text(f'{command}\ncwd={cwd}\nexit={result.returncode}\n{text}', encoding='utf-8')
    print(f'{command}: exit={result.returncode}', flush=True)
    if result.returncode or (i == 2 and 'warning:' in text):
        print(text)
        sys.exit(1)
