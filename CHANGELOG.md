# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-08-14

### Added

- A single host CLI and container shim for running coding harnesses in `astra-kali`.
- Codex, Claude Code, Pi, and OpenCode adapters with explicit API protocol selection.
- Legacy Codex Chat Completions support through the pinned `codex-chat` binary.
- Token and prompt delivery through Docker stdin instead of container arguments.
- Safe and pentest container profiles, timeout handling, JSONL event capture, and run metadata.
- Loopback URL translation, optional Docker DNS controls, dry-run output, and image diagnostics.
