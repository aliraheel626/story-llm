# story-llm

story-llm is a local-first AI storytelling desktop app built with Tauri, React,
TypeScript, and Rust. Stories, ledger entries, characters, writing-style notes,
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
    layout/
    ledger/
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
    entities/
    images/
    ledger/
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

The narrator prepares a transcript, tools, and a staged candidate, then streams
generation events. Replies owns submit, Retry, edit, and erase transactions;
Retry replaces the old reply and its effects only after successful generation.
Rolls are hidden `diceroll` ledger entries and are displayed directly from the
ledger snapshot. Images and entity projections follow their owning events.

## Local data

The SQLite database (`story-llm.sqlite3`), generated images, and API-key store
live in the operating system's Tauri application-data directory for
`com.story-llm.app`. If the new database does not yet exist, first launch copies
existing data from `com.dungeon.app`; the original files are left untouched.
If a new database already exists, it takes precedence and the old one is not
merged or overwritten. Back up both directories before any manual transfer.
Do not commit secrets or local application data.
