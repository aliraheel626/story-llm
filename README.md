# story-llm

story-llm is a local-first AI storytelling desktop app built with Tauri, React,
TypeScript, and Rust. Stories, transcript entries, characters, writing-style notes,
rolls, model settings, and generated-image metadata live in a local SQLite
database. Text generation supports OpenRouter and Nous Portal; image generation
uses OpenRouter when configured.

## Development

Prerequisites: Node.js, pnpm, a Rust toolchain, and the
[Tauri 2 platform dependencies](https://tauri.app/start/prerequisites/).

```bash
pnpm install
pnpm tauri dev
```

## Build

To produce a desktop installer or application bundle for your current platform:

```bash
pnpm install --frozen-lockfile
pnpm tauri build
```

Bundles are written under `src-tauri/target/release/bundle/`. To build just the
frontend without packaging the desktop app, run `pnpm build`.

Useful verification commands:

```bash
pnpm build
node --test src/features/story/store.test.mjs
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml export_bindings
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
```

## Architecture

The application is organized as vertical feature modules. Each feature owns its
UI, state, IPC adapter, backend commands, persistence operations, and domain
types where applicable.

```text
src/
  app/                 application shell and event wiring
  features/
    characters/
    context/
    layout/
    transcript/
    narratorTools/
    settings/
    stories/
    story/
    writingStyle/
  shared/              genuinely cross-feature types and UI

src-tauri/src/
  ai/                   shared model client and stream parsing
  features/
    compaction/
    context/
    entities/
    images/
    transcript/
    narrator/
    replies/
    settings/
    stories/
  shared/               database setup and application errors
```

Frontend feature APIs are deliberately thin wrappers around Tauri commands.
Zustand stores coordinate feature state and streamed narration events. On the
backend, Tauri commands form the feature boundary, while repositories and
feature-local helpers contain persistence and domain behavior.
The Transcript panel controls which story entries enter model history. The
Context panel manages the entity block and author's note.

The narrator prepares a transcript, tools, and a staged candidate, then streams
generation events. Replies owns submit, Retry, edit, and erase transactions;
Retry replaces the old reply and its effects only after successful generation.
Rolls are hidden `diceroll` transcript entries and are displayed directly from the
transcript snapshot. Images and entity projections follow their owning events.

## Local data

The SQLite database (`story-llm.sqlite3`, which also stores generated images)
and the API-key store (`secrets.json`) live in the operating system's Tauri
application-data directory for `com.story-llm.app`. The app creates the current
schema on a fresh database and never upgrades an older one: after a schema
change, change the database by hand (for example, drop the changed table) or
delete the `story-llm.sqlite3*` files and keep `secrets.json` to keep
the API key. Do not commit secrets or local application data.
