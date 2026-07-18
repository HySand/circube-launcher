# Implementation Plan

1. Inventory current updater call sites and freeze the Tauri/frontend contracts.
2. Extract BMCLAPI source URL construction into a typed helper with tests for metadata, Maven, assets, and invalid paths.
3. Extract the generic resource transfer engine from `updater.rs`, preserving integration-pack single-stream behavior behind an explicit policy.
4. Add retry categories, bounded backoff, response-size/Range validation, atomic commit, and stale-piece cleanup.
5. Replace the current job loop with a scheduler that separates file concurrency from global request concurrency and logs the final failed job clearly.
6. Add local HTTP integration tests for Range success, 302 redirect, 429 retry, truncated response, and Range-to-single fallback.
7. Remove duplicated download helpers and wire all client/libraries/assets/installer call sites through the engine.
8. Run `cargo fmt -- --check` (accounting for existing formatting drift), `cargo check`, `cargo test`, and `npm run build`.
9. Run the Trellis quality check and review the final diff for accidental changes to existing user work.
