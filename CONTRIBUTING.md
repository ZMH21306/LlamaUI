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
├── server/      # Process management (start/stop/monitor logs)
├── detect/      # Auto-detection of llama-server & model paths
├── init/        # Startup initialization
├── util/        # Shared utilities
├── config.rs    # Configuration persistence
├── error.rs     # Unified error types
├── events.rs    # Event names and payloads
├── log.rs       # Logging entry point
└── plugin_framework.rs  # Plugin system (experimental)
dist/            # Frontend (vanilla HTML/CSS/JS, no build step)
icons/           # Application icons
capabilities/    # Tauri permission caps
.github/         # CI/CD workflows
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
