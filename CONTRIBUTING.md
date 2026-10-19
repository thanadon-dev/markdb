# Contributing to MarkDB

Thanks for helping out. Issues and pull requests are welcome, in English or Thai.

## Reporting a bug

Open an issue with the **Bug report** template. Include:

- the MarkDB version (shown in the status bar)
- the database: PostgreSQL version, or Redshift
- the steps that cause it, what you expected, and what happened instead

Never paste real passwords, connection strings or production data into an issue.
Security problems go through [SECURITY.md](SECURITY.md), not a public issue.

## Suggesting a feature

Open an issue with the **Feature request** template first. For anything bigger than a small fix, agree on the approach in the issue before writing code, so the work isn't duplicated or rejected.

## Setting up

You need [Node.js](https://nodejs.org) 22.18+, [Rust](https://rustup.rs) stable and the [Tauri prerequisites for Windows](https://v2.tauri.app/start/prerequisites/).

```bash
npm install
npm run tauri dev
```

## Before sending a pull request

Run everything CI runs:

```bash
npm test
npm run build
cd src-tauri && cargo test
```

If you change anything that talks to the database, also run the tests against a real, throwaway Postgres:

```bash
MARKDB_TEST_PG=postgres://user:pass@localhost:5432/db cargo test -- --ignored
```

## Guidelines

- One pull request per change. Keep unrelated cleanups out of it.
- Logic that doesn't need the UI goes in `src/*.ts` with a matching `*.test.ts` next to it. Database commands go in `src-tauri/src/lib.rs` with a test.
- New behavior and bug fixes come with a test that would fail without the change.
- Writes to the database must stay safe: run them in a transaction, and check the number of affected rows where the app expects one.
- Match the style of the surrounding code. Comments explain why, not what.
- Commit messages say what changed and why, in a short first line.

## Releases

The maintainer builds, signs and publishes releases. You don't need to bump versions in a pull request.

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
