# Contributing to LlamaUI

Thank you for your interest in contributing! This document covers how to get started.

## Reporting Bugs

- Search existing [issues](https://github.com/ZMH21306/LlamaUI/issues) first.
- Include: OS version, LlamaUI version, steps to reproduce, expected vs actual behavior.
- For crash reports, attach the log file in `%LOCALAPPDATA%\LlamaUI\logs\`.

## Security Issues

**Do not file public issues for security vulnerabilities.** Email the maintainer or use [GitHub Private Vulnerability Reporting](https://github.com/ZMH21306/LlamaUI/security/advisories/new).

## Pull Requests

1. Fork and clone the repo.
2. Create a feature branch: `git checkout -b feature/your-feature`.
3. Make changes. Follow the coding guidelines below.
4. Run tests: `cargo test --lib` (must all pass).
5. Run clippy: `cargo clippy --all-targets --release` (no warnings).
6. Commit with [Conventional Commits](https://www.conventionalcommits.org/):
   ```
   feat(server): add GPU memory limit configuration
   fix(detector): handle missing CUDA toolkit gracefully
   ```
7. Push and open a PR.

## Coding Guidelines

- **No `unwrap()` / `expect()` / `panic!`** in production code.
- Errors use the unified `AppError` type from `src/error.rs`.
- Chinese error messages where user-facing; English in code comments.
- Add `#[cfg(test)]` unit tests for new logic.
- New IPC commands must update both backend (`commands/`) and frontend (`dist/main.js`).

## Project Structure

```
src/
├── commands/    # Tauri IPC commands (backend)
│   ├── server_cmd.rs       # Service process control (start/stop/restart/status)
│   ├── config_cmd.rs       # Config read/write
│   ├── config_io_cmd.rs    # Config import/export
│   ├── detect_cmd.rs       # Auto-detection (detect/cancel/check_models_dir)
│   ├── init_cmd.rs         # Startup initialization
│   ├── system_cmd.rs       # Misc (open_external_url)
│   ├── download_cmd.rs     # llama.cpp auto-download
│   ├── gpu_cmd.rs          # GPU detection & diagnosis
│   ├── hf_model_cmd.rs     # HuggingFace model store
│   ├── model_cmd.rs        # Multi-model management
│   ├── plugin_cmd.rs       # Plugin management
│   ├── recovery_cmd.rs     # Error diagnosis & auto-fix
│   ├── remote_cmd.rs       # Remote server management
│   ├── export_cmd.rs       # Log export
│   └── update_cmd.rs       # Auto-update check
├── server/      # Process management (start/stop/monitor logs)
│   ├── cmdline.rs         # Command-line parsing & path validation
│   ├── job.rs             # Windows Job Object isolation
│   ├── lifecycle.rs       # Start/stop orchestration
│   ├── log_channel.rs     # Bounded log channel with backpressure
│   ├── log_truncate.rs    # Log line truncation
│   ├── metrics.rs         # Real-time CPU/memory/GPU metrics
│   ├── mod.rs             # Server module root
│   ├── port.rs            # Port allocation & conflict resolution
│   ├── state.rs           # Server state machine
│   ├── tasks.rs           # Background task orchestration
│   └── winapi.rs          # Windows VM size query (NTAPI)
├── detect/      # Auto-detection (4-stage priority chain)
│   ├── mod.rs
│   ├── stage1.rs .. stage4.rs  # Detection stages
│   └── ctx.rs
├── init/        # Startup initialization (env check → driver check → auto load)
│   ├── mod.rs
│   ├── env_check.rs
│   ├── install_check.rs
│   └── auto_load.rs
├── util/        # Shared utilities (path/time/URL/process)
│   ├── mod.rs
│   ├── path.rs            # Path normalization, sanitize_filename, is_world_writable
│   ├── process.rs         # Silent command helper (CREATE_NO_WINDOW)
│   ├── time.rs            # Time utilities
│   └── url.rs             # URL scheme whitelist validation
├── gpu_detect.rs          # Sync GPU detection wrapper
├── gpu_detection.rs       # Async GPU detection & diagnosis (33KB)
├── gpu_error_transformer.rs  # GPU error → user-friendly messages
├── llama_downloader.rs    # llama.cpp auto-download (52KB)
├── model_management.rs    # Multi-model directory index
├── plugin_framework.rs    # Plugin system (experimental)
├── recovery.rs            # Error diagnosis & auto-fix
├── remote_server.rs       # Remote server management
├── config.rs              # Configuration persistence
├── config_io.rs           # Config JSON import/export
├── error.rs               # Unified error types
├── error_macros.rs        # Error macros
├── events.rs              # Event names + payload types
├── lib.rs                 # Crate root + Tauri Builder
├── log.rs                 # Logging entry point
├── log_sanitizer.rs       # Token/secret redaction in logs
├── main.rs                # Binary entry point (windows_subsystem)
├── tracing_setup.rs       # Structured logging setup
└── update_check.rs        # GitHub Releases update check
dist/            # Frontend (vanilla HTML/CSS/JS, no build step)
icons/           # Application icons (32x32 / 128x128 / 256x256 / .ico)
capabilities/    # Tauri permission caps
.github/         # CI/CD workflows + issue templates
scripts/         # Helper scripts (start.ps1, generate-changelog.ps1)
test/            # Test fixtures (empty; tests live in src/)
```

## Building from Source

```bash
# Prerequisites: Rust 1.70+, Node.js 18+ (for Tauri), Windows Build Tools
git clone https://github.com/ZMH21306/LlamaUI.git
cd LlamaUI

# Debug build (faster, for development)
cargo tauri dev

# Release build
cargo build --release

# Run tests
cargo test --lib

# Lint
cargo clippy --all-targets --release
```

See [README.md](README.md) for full setup instructions.
