<div align="center">

# MarkDB

**A lightweight, fast SQL client for PostgreSQL and Amazon Redshift**

[![release](https://img.shields.io/github/v/release/thanadon-dev/markdb?style=flat-square&color=222)](https://github.com/thanadon-dev/markdb/releases/latest)
[![downloads](https://img.shields.io/github/downloads/thanadon-dev/markdb/total?style=flat-square&color=222)](https://github.com/thanadon-dev/markdb/releases)
[![license](https://img.shields.io/github/license/thanadon-dev/markdb?style=flat-square&color=222)](LICENSE)
![platform](https://img.shields.io/badge/Windows-x64-222?style=flat-square)
![stack](https://img.shields.io/badge/Tauri_v2_·_Rust_·_React-222?style=flat-square)

[Download the latest version](https://github.com/thanadon-dev/markdb/releases/latest)

**English** · [ภาษาไทย](README.th.md)

</div>

---

## Why MarkDB

The installer is under 5 MB. It starts fast and uses little memory, because it runs on Tauri (the system WebView plus Rust) instead of Electron or Java.

| | Binary | RAM |
|---|---|---|
| **MarkDB** | **~8.7 MB** | **~25 MB** |
| pgAdmin | ~250 MB | ~400 MB |
| DBeaver | ~500 MB | ~700 MB |

## Supported databases

| Database | Status |
|---|---|
| PostgreSQL 12+ | All features |
| Amazon Redshift | Pick it when creating a connection; port and SSL are set for you (some features are not supported yet, see below) |

## Features

**Editing data**
- Edit a cell in place: double-click, then press `Enter`. The update goes through the primary key inside a transaction that checks exactly one row changed.
- An empty row at the bottom of each table for new data. Press `Tab` to fill in a uuid, the current time, or the column default.
- Paste from Excel or Google Sheets into a selected range of cells, or into new rows.
- Undo edits, inserted rows and `DELETE`s from the change history.

**Writing SQL**
- Autocomplete for table names, the columns of tables in `FROM`, and real values from a column when you type `where col = '`.
- A searchable history of every query you ran (`Ctrl+H`).
- Right-click a cell to filter by it (equals, not equals, contains, is NULL). Paginate, or order by from the column header.
- AI writes SQL and fixes failing queries through [Claude Code](https://claude.com/claude-code) logged in on your machine. The app never stores an API key.

**Safety**
- Mark a connection as Production or read-only. On Postgres, read-only is enforced per session with read-only transactions.
- A warning before running a write on production, and before an `UPDATE` / `DELETE` without `WHERE`.
- Delete and drop confirmations make you wait 3 seconds before the button unlocks.

**Tools**
- An ER diagram built from every foreign key in the database.
- Back up and restore a whole database as a single `.sql` file, without `pg_dump`.
- Sync tables across connections (for example dev to uat), both schema and data.
- Import and export CSV, SQL or JSON. CSV is UTF-8 with a BOM, so Excel opens it with non-Latin text intact.

**Everyday use**
- Connections are grouped by host automatically. Rename or collapse groups.
- A virtualized grid keeps thousands of rows scrolling smoothly.
- Updates itself when a new version is out.

## Install

Download `MarkDB_x.y.z_x64-setup.exe` from [Releases](https://github.com/thanadon-dev/markdb/releases/latest) and run it.

> Requires Windows x64 with WebView2 (included in Windows 11 and up-to-date Windows 10).
> The installer is not code-signed yet, so SmartScreen warns on first run. Click More info, then Run anyway.

## Known limitations

- **Connection passwords are stored in `localStorage` as plain text.** Don't use MarkDB on a machine shared with other people yet.
- Backups don't include triggers, functions, extensions, GRANTs or partitions.
- Redshift: whole-database backup isn't supported, CSV import uses `INSERT` instead of `COPY`, and read-only mode is enforced only in the app.
- Windows x64 only.

## Development

You need [Node.js](https://nodejs.org) 22.18+ (tests run `.ts` files directly), [Rust](https://rustup.rs) stable and the [Tauri prerequisites for Windows](https://v2.tauri.app/start/prerequisites/).

```bash
npm install
npm run tauri dev      # run the app with hot reload
npm run tauri build    # build the installer → src-tauri/target/release/bundle/
```

### Tests

```bash
npm test                                   # TypeScript tests (node --test)
npm run build                              # type-check and build the frontend (cargo needs dist/)
cd src-tauri && cargo test                 # Rust tests
# Tests against a real Postgres. Use a throwaway database: they create and drop tables.
MARKDB_TEST_PG=postgres://user:pass@localhost:5432/db cargo test -- --ignored
```

### Layout

| File | Purpose |
|---|---|
| `src-tauri/src/lib.rs` | Every command that talks to the database (sqlx) |
| `src-tauri/src/sync.rs` | Table sync across connections |
| `src-tauri/src/ai.rs` | Calls the `claude` CLI for AI features |
| `src/App.tsx` | UI |
| `src/*.ts` | Pure, tested logic: splitting SQL statements (`sqlsplit`), adding where/order/limit (`sqltail`), write guards (`guard`), history (`history`, `queries`) |
| `src/styles.css` | Theme |

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, code guidelines and how to send a PR.
Found a security issue? Don't open a public issue; follow [SECURITY.md](SECURITY.md).

Release notes for every version are in [CHANGELOG.md](CHANGELOG.md).

## License

[MIT](LICENSE)
