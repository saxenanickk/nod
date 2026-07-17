//! Round-trips every patch from a recorded real-world `/pulls/{n}/files`
//! response (zed-industries/zed PR #61206) through the parser and checks
//! structural invariants.

use diff_kit::{DiffLine, parse_patch};

#[test]
fn parses_every_real_patch_with_consistent_numbering() {
    let raw = include_str!("../fixtures/real_pr_files.json");
    let files: serde_json::Value = serde_json::from_str(raw).unwrap();
    let files = files.as_array().unwrap();
    assert!(!files.is_empty());

    let mut parsed_any = false;
    for file in files {
        let Some(patch) = file.get("patch").and_then(|p| p.as_str()) else {
            continue;
        };
        let name = file["filename"].as_str().unwrap();
        let hunks = parse_patch(patch).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!hunks.is_empty(), "{name}: patch produced no hunks");
        parsed_any = true;

        for hunk in &hunks {
            // Line numbers within a hunk must advance exactly with the
            // declared starts: contexts advance both sides, adds advance
            // new, removes advance old.
            let (mut old, mut new) = (hunk.old_start, hunk.new_start);
            for line in &hunk.lines {
                match line {
                    DiffLine::Context { old: o, new: n, .. } => {
                        assert_eq!((*o, *n), (old, new), "{name}: context numbering drifted");
                        old += 1;
                        new += 1;
                    }
                    DiffLine::Added { new: n, .. } => {
                        assert_eq!(*n, new, "{name}: added numbering drifted");
                        new += 1;
                    }
                    DiffLine::Removed { old: o, .. } => {
                        assert_eq!(*o, old, "{name}: removed numbering drifted");
                        old += 1;
                    }
                }
            }
        }
    }
    assert!(parsed_any, "fixture had no patches");
}
