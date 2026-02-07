# Oxcer development workflow

## Project structure

- **`oxcer-core`** — Pure Rust library: FS Service, Shell Service, Security Policy, Logging. No Tauri dependency. All core logic and unit tests live here; test with `cargo test -p oxcer-core` (or `cd oxcer-core && cargo test`).
- **`src-tauri`** — Thin Tauri launcher: initializes Tauri context/capabilities and exposes a small set of commands that delegate into `oxcer-core`. Replaceable by other frontends (e.g. SwiftUI, Flutter) that reuse the same core.

## Default workflow

- **Core logic:** Change code in `oxcer-core`; run tests there first.
  - `cd oxcer-core`
  - `cargo test`
  - No Tauri or window; tests are pure Rust.

- **Tauri app sanity:** After core tests are green, ensure the launcher still builds and runs.
  - `cd src-tauri`
  - `cargo test` — compiles the binary (with mock context); no need to run integration tests here if all logic is in `oxcer-core`.
  - `cargo tauri dev` — for manual integration and UI.

- **Routine:** Modify core → run `cargo test` in `oxcer-core` → then run `cargo tauri dev` when you need to try the full app.

## Tauri context and `cargo test`

- **Do not** call `tauri::generate_context!()` directly from code that is compiled when running `cargo test`.
- `generate_context!()` depends on `OUT_DIR` from the Tauri build script and fails when the test binary is built.
- **Fix:** Use the `app_context()` helper in `src-tauri/src/main.rs`:
  - `#[cfg(test)]`: returns `tauri::test::mock_context(tauri::test::noop_assets())` so the test binary compiles.
  - `#[cfg(not(test))]`: returns `tauri::generate_context!()` for real runs.
- In `main()` we call `.run(app_context())` instead of `.run(tauri::generate_context!())`.

## Security and capabilities

- **FS and Shell** go through our own services and **Security Policy** (path blocklist, command deny list); they are not raw filesystem/shell access.
- Capabilities are configured in `tauri.conf.json` under `app.security.capabilities`; the main window uses the `"main"` capability.
- For new features, add tests in `oxcer-core` (in the relevant module: `fs.rs` or `shell.rs`) first; keep Tauri-specific wiring in `src-tauri/src/main.rs` and validate with `cargo tauri dev` when needed.

## Keeping the loop tight

- Fix the **first 1–3** errors from `cargo test` or `cargo build`, then re-run; avoid fixing many errors in one go.
- Do not paste LLM meta tokens (`<think>`, `<|tool_calls_begin|>`, etc.) into `.rs` files; keep design notes in `.md` and only validated Rust in source.
