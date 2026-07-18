# Design: Non-pack Download Engine and Updater Architecture

## Boundaries

```text
manifest/version metadata
        |
        v
source URL policy  --->  download scheduler  --->  transfer strategy
        |                         |                      |
        v                         v                      v
BMCLAPI URL contract       file/request budgets    single or Range stream
                                                           |
                                                           v
                                                temp files + SHA-1 validation
                                                           |
                                                           v
                                                atomic destination commit
```

## Proposed modules

- `download_sources.rs`: typed source policy and BMCLAPI path rewrites. The module must distinguish metadata, Maven, and asset-object paths.
- `download_engine.rs`: job model, bounded scheduler, retry classification/backoff, Range-piece orchestration, temporary-file cleanup, and atomic commit.
- `download_progress.rs` (or a small internal module): byte counters, speed monitor, and progress event emission.
- `updater.rs`: manifest/resource orchestration only; it builds typed jobs and invokes the engine.

If the user requires a one-file change, retain these boundaries as private submodules or clearly separated sections, but the recommended implementation is physical modules.

## Download policy

- `DownloadPolicy::Pack`: single stream only, existing source-generation cancellation semantics.
- `DownloadPolicy::Resource`: single stream only, with a global request budget across files.
- Range is used only to resume an existing single-stream temporary file. Parallel pieces are forbidden for every source.

## Retry and error contract

Errors carry destination, URL, HTTP status when available, attempt number, and a stable category (`Transient`, `RateLimited`, `RangeUnsupported`, `Integrity`, `Permanent`, `Canceled`).

- Retry `408`, `429`, `500`, `502`, `503`, `504`, transport resets, and truncated bodies with exponential backoff and jitter-free bounded delays.
- Do not retry malformed metadata, a stable 404, or a SHA-1 mismatch after a clean single-stream retry without cleaning the temporary state.
- Never silently reinterpret a 404 as a different URL shape.

## Compatibility

- Keep `sync_versions`, `get_manifest_versions`, and frontend event names unchanged.
- Keep `.minecraft` layout unchanged.
- Continue SHA-1 validation for every job that provides a hash.
- Preserve the explicit no-official-source constraint.
