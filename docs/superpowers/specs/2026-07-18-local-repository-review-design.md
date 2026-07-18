# Local repository review — design

## Context

nod today reviews GitHub PRs only: you land on a global PR queue, open one,
and review its diff with inline threads. This adds the ability to **open any
local repository** and either review its local changes (no PR) or jump to that
repo's PRs. It removes the need to already have a PR to look at a diff in the
app, and turns nod into a pre-push self-review tool as well as a PR review
tool.

## Goals

- Open a local clone and **review its diff with no PR involved** — either the
  current branch vs its base, or uncommitted changes.
- From a local repo, **jump into that repo's PRs** (the existing PR experience,
  scoped to the repo).
- Reuse the existing diff experience (file sidebar, unified/side-by-side,
  syntax highlighting, `]`/`[` nav, ⌘-click to Zed) for local review.

## Non-goals (v1)

- Local review is **read-only**: no comment threads, no composer, no merge.
  There is no PR to attach comments to; the workflow is review → jump to Zed to
  edit. (Leaving notes-to-self is a possible later addition.)
- No staging/committing/git actions from the app beyond reading diffs.
- No remote hosts other than GitHub for the PRs action.

## User-facing behavior

### Navigation

The window gains a top-level toggle: **PRs** (today's global queue, unchanged)
and **Repos** (new). The Repos view is a `RepoPickerView` listing local clones:

- Auto-discovered by scanning `config.clone_roots` (depth 3) plus any paths in
  `config.clones`, deduplicated, each resolved to its `owner/name` via the
  origin remote (reusing `repo_local::origin_repo` / `find_clone`).
- Each row shows the repo name + path and two actions:
  - **Review local changes** → opens a `LocalSessionView` for that clone.
  - **View PRs** → opens the PR list scoped to `repo:owner/name`.
- An **"Open folder…"** button opens a native folder picker (GPUI
  `cx.prompt_for_paths`) for a clone not in the list.

CLI: `nod <path>` opens that repo's local review directly (bypassing the
picker). A non-existent path prints an error and opens the normal window.

### Local review session

Same diff surface as PR review, minus the PR-only bits:

- Header: `repo-name · branch ← base`, with a **mode toggle** (Branch vs base /
  Uncommitted), the **Split/Unified** toggle, **Refresh**, and **Open repo in
  Zed**.
- Body: the shared diff pane — file sidebar (status marker + `+`/`−` stats, no
  comment badge), and the unified or side-by-side diff with syntax
  highlighting.
- ⌘-click a line opens it in Zed (the clone is by definition on the branch, so
  line numbers are trustworthy in Uncommitted / Branch-head modes).
- Keyboard: `]`/`[` next/prev file, `r` refresh, `esc` back. (`n`/`p` thread
  nav is a no-op here.)

Two local diff modes:

- **Branch vs base** (default): `git diff <base>...<head>` (three-dot =
  merge-base diff, matching what a PR would show). `base` auto-detected from
  `origin/HEAD` (falls back to `main`, then `master`); overridable via a small
  text field in the header.
- **Uncommitted**: `git diff HEAD` (staged + unstaged vs HEAD).

Empty diff (clean tree / no divergence) shows a friendly "no changes" state.

## Internal structure (Approach A: shared diff pane)

### `diff_kit` — `parse_git_diff`

```rust
pub fn parse_git_diff(diff: &str) -> Vec<FileDiff>;
```

Parses full `git diff` output into the existing `FileDiff` model:

- Splits on `diff --git a/… b/…` file boundaries.
- Per file: reads `new file mode` / `deleted file mode` / `rename from|to`
  and `Binary files … differ` markers to set `FileStatus`, `old_path`, and
  `is_binary_or_too_large`.
- Reuses the existing hunk parser on each file's `@@ … @@` section (extracted
  as `parse_hunks` so both `parse_patch` and `parse_git_diff` share it).

UI-free; fixture-tested against recorded real `git diff` output covering
modify, add, delete, rename, and binary.

### `repo_local` — local diff helpers

```rust
pub fn default_base(clone: &Path) -> Result<String>;      // origin/HEAD → main → master
pub fn branch_diff(clone: &Path, base: &str) -> Result<String>;   // git diff base...HEAD
pub fn uncommitted_diff(clone: &Path) -> Result<String>;          // git diff HEAD
```

All shell out to `git` via the existing helper, consistent with checkout.

### `diff_pane` module (new, in `nod`)

The shared diff rendering, lifted out of `PrSessionView`:

- Free functions already at module scope (`code_cell`, `highlight_style`,
  `render_diff_line`, `render_half`, `outdated_header_el`, `anchor_for_line`)
  move here.
- `render_file_header`, `render_hunk_header`, and the file-sidebar row move
  here as free functions taking plain data (`&[FileDiff]`, stats,
  `zed_target`, an optional `badge: Option<u32>`).
- PR-only interactivity becomes optional hooks passed by the caller:
  - `add_comment: Option<Rc<dyn Fn(CommentAnchor, &mut Window, &mut App)>>` —
    drives the `+`-to-comment affordance; `None` in local mode (no `+`).
  - thread/outdated rows are rendered by a caller-provided
    `render_thread: Option<Rc<dyn Fn(usize, &NodeId) -> AnyElement>>`; local
    rows never contain thread variants, so `None` is safe.
- A `render_diff_body(...)` helper builds the sidebar + `list()` layout given a
  row-dispatch closure, so both sessions share the wiring.

`ViewMode` (Unified/Split) moves to `diff_pane` since both sessions use it.

### `PrSessionView` refactor

Renders through `diff_pane`, passing `Some(add_comment)` and
`Some(render_thread)`. Its threads/composer/review-drawer/merge/checkout code
is unchanged. Verified post-refactor by driving the running app against a real
PR.

### `LocalSessionView` (new)

Small. Holds clone path, repo id, base, current branch, mode, `files`,
`view_mode`, `selected_file`, `list_state`, `sidebar_scroll`, load state, focus.
Fetches the git diff on the background executor, parses via
`diff_kit::parse_git_diff`, and renders through `diff_pane` with no PR hooks.
Reuses the existing `SESSION_KEY_CONTEXT` bindings.

### `Workspace` + `RepoPickerView`

`Workspace` grows a `Tab { Prs, Repos }` and renders the PRs/Repos toggle. It
holds `pr_list`, an optional `pr_session`, an optional `local_session`, and a
`repo_picker`. `RepoPickerView` emits events: `OpenLocal(path)` and
`OpenRepoPrs(RepoId)`; the workspace opens the corresponding session (for
`OpenRepoPrs` it re-scopes and shows the PR list).

## Isolation

`parse_git_diff` and the `repo_local` helpers are UI-free. Only `nod`
touches gpui (`diff_pane`, `LocalSessionView`, `RepoPickerView` all live in the
app crate). `scripts/check-gpui-isolation.sh` continues to enforce this.

## Milestones

- **L1** — `diff_kit::parse_git_diff` (+ shared `parse_hunks`) and the
  `repo_local` diff/base helpers, with tests. No UI.
- **L2** — extract `diff_pane`; refactor `PrSessionView` onto it; verify the PR
  path still works.
- **L3** — `LocalSessionView`; open it via a temporary path (e.g. CLI arg) to
  test end-to-end against a real local repo.
- **L4** — `RepoPickerView`, the PRs/Repos navigation, the folder picker, and
  the repo-scoped PRs action.

## Verification

- `cargo test` for `parse_git_diff` (fixtures: modify/add/delete/rename/binary)
  and a `repo_local` integration test creating a real temp repo with a branch.
- Drive the running app: open a real local clone (this repo), confirm branch
  and uncommitted diffs render with highlighting; confirm the PR path is
  unregressed; confirm ⌘-click opens the right line in Zed.
