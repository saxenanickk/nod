# prdesk

A fast, native GitHub pull-request review app for people who edit in [Zed](https://zed.dev).

Zed's extension API has no UI extension points, so a VS Code-style PR review
experience can't live inside Zed today. prdesk is the next best thing: a
standalone [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
app you keep next to Zed —

- see the PRs waiting on you, check one out locally with one click,
- read the diff with review comment threads inline,
- comment, reply, resolve, approve/request changes, merge,
- review a **local repository's** diff (branch-vs-base or uncommitted) with no
  PR involved, and
- jump from any diff line straight into Zed (`zed file:line`) with full LSP.

Built in GPUI (the same Rust UI framework Zed uses) with UI-free domain
crates, so the core could one day move into Zed itself.

## Two ways in

The top nav switches between:

- **PRs** — your review queue (to review / created / assigned). Open a PR to
  review it: threads, comments, approve/request-changes, merge.
- **Repos** — your local clones (auto-discovered under `clone_roots`, plus
  anything in `clones`; or **Open folder…**). Each offers **Review local
  changes** (a read-only diff of the current branch vs its base, or
  uncommitted changes — toggle in the header) and **View PRs** (the PR queue
  scoped to that repo).

Or open a local review straight from the shell: `prdesk ~/path/to/repo`.

## Requirements

- macOS
- [`gh`](https://cli.github.com) installed and authenticated (`gh auth login`)
- the `zed` CLI (Zed → install CLI) for jump-to-editor

## Configuration

`~/Library/Application Support/prdesk/config.json` (created on first run):

```json
{
  "gh_user": "saxenanickk",
  "scope": "org:your-org",
  "clones": { "owner/repo": "/path/to/local/clone" },
  "poll_seconds": 60
}
```

- `gh_user` — which gh account to act as (gh supports multiple); `null` uses
  gh's active account.
- `scope` — extra search qualifier for the PR list (`org:…` / `repo:…`);
  empty searches all of GitHub.

## Keyboard shortcuts

In an open PR:

| Key | Action |
|-----|--------|
| `]` / `[` | Next / previous file |
| `n` / `p` | Next / previous comment thread |
| `r` | Refresh |
| `esc` | Close the composer/review drawer, else go back |
| `cmd-q` | Quit |

Use the **Split** / **Unified** button in the PR header to switch between a
side-by-side and an inline diff.

## Development

```sh
cargo run          # the app
cargo test         # domain-crate tests
```

Workspace rule: only the `prdesk` crate may depend on gpui/gpui-component
(`cargo tree -i gpui` must show a single crate). Everything below it —
`github_types`, `github_client`, `diff_kit`, `review_store` — stays UI-free.

### Packaging a macOS `.app`

```sh
cargo install cargo-bundle
cargo bundle --release --package prdesk
```
