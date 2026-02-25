# Project Identity & Quality Ethos

This is a production-grade codebase, not a prototype. Agents amplify patterns — quality is non-negotiable.

> "This codebase will outlive you. Every shortcut you take becomes someone else's burden. Every hack compounds into technical debt that slows the whole team down. You are not just writing code. You are shaping the future of this project. The patterns you establish will be copied. The corners you cut will be cut again. Fight entropy. Leave the codebase better than you found it."

## Feedback Loops

- Run ALL checks before committing: typecheck, test, lint
- Do not commit if any check fails. Fix first.
- Run checks after every meaningful change, not just at the end

## TDD: Red-Green-Refactor

All new functionality must have tests. Follow the cycle:

1. **Red** — Write a failing test first
2. **Green** — Write minimal code to make it pass
3. **Refactor** — Clean up while keeping tests green

Do not skip steps. Do not write implementation before the test.
