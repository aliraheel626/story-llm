# Dungeon

Dungeon is a local-first AI storytelling desktop app built with Tauri, React,
TypeScript, and Rust. Stories, passages, characters, writing-style notes, dice rolls,
model settings, and generated-image metadata are persisted in a local SQLite
database. Text and image generation use OpenRouter when configured.

## Development

Prerequisites: Node.js, pnpm, the Rust toolchain, and the platform dependencies
required by Tauri 2.

```bash
pnpm install
pnpm tauri dev
```

Useful verification commands:

```bash
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml export_bindings
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
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
    dicerolls/
    passages/
    settings/
    stories/
    writingStyle/
  shared/              genuinely cross-feature types and UI

src-tauri/src/
  ai/                   shared narration client and stream parsing
  features/
    entities/
    images/
    dicerolls/
    passages/
    settings/
    stories/
  shared/               database setup and application errors
```

Frontend feature APIs are deliberately thin wrappers around Tauri commands.
Zustand stores coordinate feature state and streamed narration events. On the
backend, Tauri commands form the feature boundary, while repositories and
feature-local helpers contain persistence and domain behavior.

Passage generation is streamed to the UI and persisted only after successful
completion. Retrying replaces the existing narrator passage transactionally,
so configuration or generation failures leave the previous passage intact.
Narration and dice-roll writes share one transaction, and mutations also
refresh the owning story's `updated_at` value.

## Local data

The SQLite database and generated images live in the operating system's Tauri
application-data directory. OpenRouter API keys are stored through the app's
settings commands; do not commit secrets or local application data.
