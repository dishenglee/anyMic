# Contributing to anyMic

Thank you for your interest in contributing! Please read this guide before opening a PR.

## Getting Started

1. Fork the repository and create a feature branch from `main`.
2. Make your changes with clear, focused commits.
3. Open a pull request against `main` and fill in the PR template.

## Commit Convention

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short summary>
```

Allowed types: `feat`, `fix`, `refactor`, `docs`, `chore`, `test`, `ci`, `perf`.

Examples:
- `feat(server): implement Opus UDP receive loop`
- `fix(android): handle audio focus loss during streaming`
- `docs(readme): add installation instructions for macOS`

Rules:
- Summary is lowercase, no trailing period, ≤ 72 characters.
- Use the imperative mood ("add" not "added").
- Reference issue numbers in the body when relevant: `Closes #42`.

## Pull Request Guidelines

- Keep PRs small and focused on a single concern.
- All CI checks must pass before merging.
- Add or update tests for any changed behaviour.
- Update relevant documentation if the user-facing behaviour changes.

## Code Style

- **Rust**: run `cargo fmt` and `cargo clippy -- -D warnings` before committing.
- **Kotlin**: follow the [Kotlin coding conventions](https://kotlinlang.org/docs/coding-conventions.html). Use `ktlint` if available.
- **General**: respect `.editorconfig` (UTF-8, LF line endings, trailing newline).

## Reporting Issues

Use GitHub Issues. Include steps to reproduce, OS version, app version, and relevant logs.
