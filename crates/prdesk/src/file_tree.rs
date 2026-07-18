//! Builds the changed-files sidebar as either a flat list or a collapsible
//! folder tree. Pure (no gpui) so the layout logic is unit-testable; the
//! session views render the resulting rows.

use diff_kit::FileDiff;
use std::collections::HashSet;

/// One rendered row in the sidebar: a folder header or a file entry. `depth`
/// is the indentation level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarRow {
    Folder {
        depth: usize,
        /// Display name (possibly a compacted `a/b/c` chain).
        name: String,
        /// Full path from the root, used as the collapse key.
        path: String,
        /// Files under this folder (recursive), for the count badge.
        file_count: usize,
        collapsed: bool,
    },
    File {
        depth: usize,
        file_ix: usize,
    },
}

#[derive(Default)]
struct Node {
    /// Child folders, in first-seen order.
    folders: Vec<(String, Node)>,
    /// Files directly at this level (indices into the original slice).
    files: Vec<usize>,
}

impl Node {
    fn child_mut(&mut self, name: &str) -> &mut Node {
        if let Some(pos) = self.folders.iter().position(|(n, _)| n == name) {
            &mut self.folders[pos].1
        } else {
            self.folders.push((name.to_string(), Node::default()));
            &mut self.folders.last_mut().unwrap().1
        }
    }

    fn file_count(&self) -> usize {
        self.files.len() + self.folders.iter().map(|(_, c)| c.file_count()).sum::<usize>()
    }
}

/// Builds the sidebar rows. In flat mode every file is a depth-0 `File` in the
/// original order. In tree mode files are grouped by directory, single-child
/// folder chains are compacted (`a/b/c` → one row), and folders listed in
/// `collapsed` hide their descendants.
pub fn build_rows(files: &[FileDiff], collapsed: &HashSet<String>, tree: bool) -> Vec<SidebarRow> {
    if !tree {
        return (0..files.len()).map(|file_ix| SidebarRow::File { depth: 0, file_ix }).collect();
    }
    let mut root = Node::default();
    for (file_ix, file) in files.iter().enumerate() {
        let comps: Vec<&str> = file.path.split('/').collect();
        let mut node = &mut root;
        // All but the last component are folders; the last is the file name.
        for comp in &comps[..comps.len().saturating_sub(1)] {
            node = node.child_mut(comp);
        }
        node.files.push(file_ix);
    }
    let mut out = Vec::new();
    flatten(&root, "", 0, collapsed, &mut out);
    out
}

/// Repo-relative paths of every file in `folder`'s subtree (recursive).
pub fn files_under(files: &[FileDiff], folder: &str) -> Vec<String> {
    let prefix = format!("{folder}/");
    files
        .iter()
        .filter(|f| f.path.starts_with(&prefix))
        .map(|f| f.path.clone())
        .collect()
}

fn flatten(
    node: &Node,
    prefix: &str,
    depth: usize,
    collapsed: &HashSet<String>,
    out: &mut Vec<SidebarRow>,
) {
    // Folders first, then files at this level.
    for (name, child) in &node.folders {
        // Compact a chain of single-child folders with no files of their own.
        let mut full = name.clone();
        let mut cur = child;
        while cur.files.is_empty() && cur.folders.len() == 1 {
            let (n, c) = &cur.folders[0];
            full = format!("{full}/{n}");
            cur = c;
        }
        let path = if prefix.is_empty() { full.clone() } else { format!("{prefix}/{full}") };
        let is_collapsed = collapsed.contains(&path);
        out.push(SidebarRow::Folder {
            depth,
            name: full,
            path: path.clone(),
            file_count: cur.file_count(),
            collapsed: is_collapsed,
        });
        if !is_collapsed {
            flatten(cur, &path, depth + 1, collapsed, out);
        }
    }
    for &file_ix in &node.files {
        out.push(SidebarRow::File { depth, file_ix });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diff_kit::{FileDiff, FileStatus};

    fn file(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary_or_too_large: false,
            hunks: Vec::new(),
        }
    }

    fn files(paths: &[&str]) -> Vec<FileDiff> {
        paths.iter().map(|p| file(p)).collect()
    }

    #[test]
    fn flat_mode_preserves_order() {
        let fs = files(&["b/y.rs", "a/x.rs"]);
        let rows = build_rows(&fs, &HashSet::new(), false);
        assert_eq!(
            rows,
            vec![
                SidebarRow::File { depth: 0, file_ix: 0 },
                SidebarRow::File { depth: 0, file_ix: 1 },
            ]
        );
    }

    #[test]
    fn groups_by_folder() {
        let fs = files(&["src/a.rs", "src/b.rs", "top.rs"]);
        let rows = build_rows(&fs, &HashSet::new(), true);
        assert_eq!(
            rows,
            vec![
                SidebarRow::Folder {
                    depth: 0,
                    name: "src".into(),
                    path: "src".into(),
                    file_count: 2,
                    collapsed: false
                },
                SidebarRow::File { depth: 1, file_ix: 0 },
                SidebarRow::File { depth: 1, file_ix: 1 },
                SidebarRow::File { depth: 0, file_ix: 2 },
            ]
        );
    }

    #[test]
    fn compacts_single_child_chains() {
        let fs = files(&["modules/app/src/hooks/use.ts"]);
        let rows = build_rows(&fs, &HashSet::new(), true);
        assert_eq!(
            rows,
            vec![
                SidebarRow::Folder {
                    depth: 0,
                    name: "modules/app/src/hooks".into(),
                    path: "modules/app/src/hooks".into(),
                    file_count: 1,
                    collapsed: false
                },
                SidebarRow::File { depth: 1, file_ix: 0 },
            ]
        );
    }

    #[test]
    fn collapsed_folder_hides_children() {
        let fs = files(&["src/a.rs", "src/b.rs"]);
        let mut collapsed = HashSet::new();
        collapsed.insert("src".to_string());
        let rows = build_rows(&fs, &collapsed, true);
        assert_eq!(
            rows,
            vec![SidebarRow::Folder {
                depth: 0,
                name: "src".into(),
                path: "src".into(),
                file_count: 2,
                collapsed: true
            }]
        );
    }

    #[test]
    fn nested_folders_split_when_branching() {
        let fs = files(&["a/b/x.rs", "a/c/y.rs"]);
        let rows = build_rows(&fs, &HashSet::new(), true);
        // `a` has two children, so it is not compacted; `a/b` and `a/c` each
        // hold one file.
        assert_eq!(
            rows,
            vec![
                SidebarRow::Folder {
                    depth: 0,
                    name: "a".into(),
                    path: "a".into(),
                    file_count: 2,
                    collapsed: false
                },
                SidebarRow::Folder {
                    depth: 1,
                    name: "b".into(),
                    path: "a/b".into(),
                    file_count: 1,
                    collapsed: false
                },
                SidebarRow::File { depth: 2, file_ix: 0 },
                SidebarRow::Folder {
                    depth: 1,
                    name: "c".into(),
                    path: "a/c".into(),
                    file_count: 1,
                    collapsed: false
                },
                SidebarRow::File { depth: 2, file_ix: 1 },
            ]
        );
    }
}
