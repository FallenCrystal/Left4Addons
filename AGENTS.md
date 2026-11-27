# Repository Guidelines

## Project Structure & Module Organization

- `src/` contains the React/TypeScript UI. Keep desktop IPC calls in `src/api/ipc.ts`. Frontend assets live in `src/assets/` and `public/`, desktop icons in `src-tauri/icons/`, and screenshots in `preview/`.
- `src-tauri/` initializes the engine and forwards IPC commands. Keep application logic and local SQLite storage in `crates/l4a-core/`, shared DTOs and events in `crates/l4a-protocol/`, and logging setup plus network/process audit helpers in `crates/l4a-logging/`.
- Other crates isolate VPK parsing, Steam integration, mirror routing, and Workshop providers. The standalone Axum/SQLite catalog service lives in `crates/l4a-server/`. See [the architecture index](docs/README.md) for crate boundaries and API contracts.

## Build, Test, and Development Commands

Run these from the repository root.

- `npm ci` installs dependencies from the lockfile. `npm run dev` starts the browser preview, and `npm run tauri:dev` starts the desktop app with IPC.
- `npm run build` builds the frontend into `dist/`. `npm run tauri:build` builds the desktop app.
- `cargo test` tests the eight default workspace crates. To target one crate, run `cargo test -p l4a-core`. `npm test` runs Vitest once.
- `L4A_SERVER_ADMIN_TOKEN=dev-secret cargo run -p l4a-server` starts the local catalog service at `http://127.0.0.1:8787`.

## Coding Style & Naming Conventions

Use four spaces for Rust, with `snake_case` modules and functions and `PascalCase` types. Format Rust with `cargo fmt --all` and check it with `cargo clippy --all-targets -- -D warnings`. In React source, use two spaces, single quotes, `PascalCase` components, and `camelCase` functions. Run `npm run lint` for Oxlint.

## Testing Guidelines

Keep Rust unit tests alongside code and integration tests in crate `tests/` directories, following names such as `engine_integration.rs`. Place Vitest tests next to the modules they cover, as in `src/api/ipc.test.ts`. Use `MockSteamBridge`, temporary directories and VPKs, and local HTTP fixtures to keep tests independent of Steam and external services. The repository specifies no numeric coverage threshold.

## Commit & Pull Request Guidelines

Follow the history's short Conventional Commit subjects, with an optional scope, such as `fix(security): enhance security hardening and CSP image loading`. In pull requests, summarize behavior changes, list verification commands, link related issues when available, and include screenshots for visible UI changes.
