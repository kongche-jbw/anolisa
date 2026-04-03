# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **cosh**: Migrate config directory from ~/.copilot to ~/.copilot-shell
- **cosh**: Add nvm-aware Node.js detection in cosh wrapper
- **cosh**: Install files per FHS directory layout
- **cosh**: Support loading skills from extension (#55)
- **cosh**: Add system-level install via Makefile and align spec
- **cosh**: Add cosh-extension.json compatibility
- **cosh**: Add built-in cd command support for changing directory (#19)
- **cosh**: Detect OpenClaw configured api-key on bootstrap
- **cosh**: Add session renaming command
- **cosh**: Register cosh and copilot aliases in create_alias.sh
- **cosh**: Support multiple custom providers

### Fixed

- **cosh**: Reduce TUI flicker on Qwen OAuth page in limited-height terminals
- **cosh**: Allow left arrow to wrap from line start to previous line end
- **cosh**: Add 30s auto-accept timeout to create_alias.sh interactive prompt
- **cosh**: Lock package for avoiding lint failure

