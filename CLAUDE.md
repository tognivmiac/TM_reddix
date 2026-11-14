# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Reddix is a Rust terminal Reddit client (ratatui + crossterm) with inline image previews via the kitty graphics protocol and video playback via mpv. Single crate (Rust 2024 edition, `rust-version = "1.100"`), binary + library (`src/main.rs` is a thin CLI-flag shim over `reddix::run()` in `src/app.rs`). This is a private personal fork: the installed command is `redx` (`[[bin]]` in `Cargo.toml`, `BIN_NAME` in `src/lib.rs`), while the package, library crate and config/cache directories keep the `reddix` name. Install with `cargo install --path . --locked`. Fork versions are `<upstream base>+tm.<YYYYMMDD>` with the date in UTC (`date -u +%Y%m%d`); bump `Cargo.toml`, `CHANGELOG.md` and `docs/release-notes.yaml` together.

## Commands

```sh
cargo build                          # debug build
cargo run                            # launch the TUI
cargo run -- --version | --help | --check-updates
cargo test                           # unit tests (in-module #[cfg(test)]) + tests/version_flag.rs
cargo test <name>                    # single test by name substring, e.g. cargo test prints_version
cargo test --lib config::            # tests in one module
cargo clippy --all-targets
cargo fmt
```

Releases are built by cargo-dist (`dist-workspace.toml`, `.github/workflows/release.yml`, generated — regenerate with `dist init`/`dist generate` rather than hand-editing). Pushing a semver tag (e.g. `v0.2.10`) triggers the release. The CI workflow only runs dist planning on PRs; there is no separate test/lint CI.

When cutting a release, bump `Cargo.toml` version and update both `CHANGELOG.md` (Keep a Changelog format) and `docs/release-notes.yaml`. The latter is compiled into the binary via `include_str!` in `src/release_notes.rs` and drives the in-app "What's new" banner, so it must stay valid YAML with semver `version` fields.

## Architecture

Startup (`app::run`): load config → open SQLite `storage::Store` → start `media::Manager` workers → if Reddit credentials are configured, build `auth::Flow` → `session::Manager` (loads stored accounts/tokens) → `reddit::Client` with the active session's token provider → wrap it in the `data::*Service` traits → hand everything to `ui::Model::new(options).run()`. Without credentials/session, the UI runs with placeholder posts and services set to `None`.

Layers:
- `config.rs` — YAML config (`~/.config/reddix/config.yaml`, `%APPDATA%/Reddix` on Windows; example in `docs/examples/config.yaml`) merged with env overrides `REDDIX_<SECTION>__<KEY>` (double underscore = `.`). New config keys need handling in both `merge_config` and `apply_env_value`.
- `auth.rs` — OAuth2 authorization-code flow with a local `tiny_http` callback server on the redirect URI (default `http://127.0.0.1:65010/reddix/callback`); token refresh.
- `session.rs` — multi-account management on top of auth + storage.
- `storage.rs` — rusqlite (bundled) with WAL; holds accounts, tokens, media cache index, and `app_state` key/values. Schema migrations in `migrate()` are versioned by their position in the migrations list (recorded in `schema_migrations`), so only append new migrations — never reorder or edit existing ones.
- `reddit.rs` — blocking `reqwest` API client and serde models (`Post`, `Listing`, sort options).
- `data.rs` — `FeedService`/`SubredditService`/`CommentService`/`InteractionService` traits with Reddit-backed and `Mock*` implementations. The UI depends only on these traits.
- `media.rs` — background worker pool (crossbeam channels) that downloads and caches media on disk with TTL/size limits; UI talks to it via `media::Handle`.
- `video.rs` — mpv launching (inline kitty mode with IPC socket, or external/detached).
- `markdown.rs` — pulldown-cmark → ratatui `Text` rendering.
- `update.rs` — personal-fork update flow: "latest" is the `[package] version` in the local source checkout the binary was built from (`CARGO_MANIFEST_DIR`, overridable with `REDDIX_SOURCE_DIR`), not an upstream release. Self-update runs `cargo build --release` there and swaps the running executable. No network access.
- `ui.rs` — ~13k lines; the entire TUI state machine in `Model`.

### UI concurrency model

Everything network/IO-bound is blocking and runs on `std::thread::spawn` from inside `ui.rs`. Each worker sends an `AsyncResponse` variant back over a crossbeam channel; `Model::event_loop` polls it each tick (`poll_async`), and most responses carry a `request_id` so stale results from superseded requests are discarded. Rendering is dirty-flag driven (`mark_dirty` / `needs_redraw`). Kitty image/video output is written directly to the backend after `terminal.draw` (`flush_inline_images`, `flush_inline_clears`), outside ratatui's buffer — changes to layout must keep these in sync.

To add a new async operation: add an `AsyncResponse` variant, spawn the thread with a cloned sender, and handle the variant in `poll_async`.

## Debug / override env vars

- `REDDIX_SKIP_UPDATE_CHECK`, `REDDIX_FORCE_UPDATE_VERSION`, `REDDIX_FORCE_UPDATE_URL`, `REDDIX_OVERRIDE_CURRENT_VERSION`, `REDDIX_SOURCE_DIR` — update-flow testing
- `REDDIX_DISABLE_KITTY`, `REDDIX_FORCE_KITTY`, `REDDIX_ENABLE_KITTY`, `REDDIX_DEBUG_KITTY` — kitty graphics detection (also checks `TMUX`, `KITTY_WINDOW_ID`, `TERM`, `TERM_PROGRAM`)
- `REDDIX_DEBUG_VIDEO`, `REDDIX_DEBUG_VIDEO_LOG`, `REDDIX_MPV_PATH`, `REDDIX_MPV_USER_AGENT`, `REDDIX_MPV_REFERER` — mpv/video
