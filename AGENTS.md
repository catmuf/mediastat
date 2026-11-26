# MediaStat Repository Guidelines

## Repository Language (English Only)
- Keep all repository content in **English**: git commit messages, `README.md`, documentation, CLI help text, and code comments.
- Exception: Intentional runtime internationalization (`ES` / `EN`) strings inside `mediastat_core::t(...)` / `t_for(...)` and `i18n` tests must preserve both Spanish and English translations.

## Git Commits & README Synchronization
- Group changes into clean, logical, atomic commits using English Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`, `test:`).
- Before creating **every commit**, check whether `README.md` needs to be updated to reflect the changes in that commit (new or removed features, CLI flags/subcommands, keyboard shortcuts, architecture, or build instructions).
- Include any required `README.md` updates in the same commit so documentation stays in sync at every point in git history.

## Remote & Pushing
- Primary remote (`origin`): `git@github.com:catmuf/mediastat.git`
- Default branch: `main`
- Before pushing Rust code changes, verify the workspace builds and passes tests (`cargo test --workspace`) and check `git status` to ensure no build artifacts or temporary files are staged.
