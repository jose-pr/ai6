//! The `apply_patch` text, for a codec that turns a file change of one harness into another's.
//!
//! Only what two harnesses can say alike goes across: a whole file written, and one run of lines
//! replaced. Anything else is not a [`FileOp`], and the codec writes the call as a note.

/// One change to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileOp {
    /// A file written whole.
    Add {
        /// Its path, as the patch spells it.
        path: String,
        /// Its text, every line ended by a line feed.
        content: String,
    },
    /// One run of lines replaced.
    Update {
        /// The file's path, as the patch spells it.
        path: String,
        /// The lines replaced, context included, joined by line feeds.
        old: String,
        /// What replaces them.
        new: String,
    },
}

/// A patch that writes `content` to `path`.
#[must_use]
pub fn add(path: &str, content: &str) -> String {
    let mut patch = format!("*** Begin Patch\n*** Add File: {path}\n");
    for line in content.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch.push_str("*** End Patch\n");
    patch
}

/// A patch that replaces the lines `old` with `new` in `path`.
#[must_use]
pub fn update(path: &str, old: &str, new: &str) -> String {
    let mut patch = format!("*** Begin Patch\n*** Update File: {path}\n@@\n");
    for line in old.lines() {
        patch.push('-');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in new.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch.push_str("*** End Patch\n");
    patch
}

/// The one change a patch makes, when it makes exactly one of the two kinds [`FileOp`] holds: a file
/// added, or one file updated by one hunk with something to replace. Otherwise `None`.
#[must_use]
pub fn parse(patch: &str) -> Option<FileOp> {
    let mut lines = patch.lines();
    if lines.next()?.trim_end() != "*** Begin Patch" {
        return None;
    }
    let mut op: Option<FileOp> = None;
    let mut hunks = 0;
    let mut old: Vec<&str> = Vec::new();
    let mut new: Vec<&str> = Vec::new();
    let mut ended = false;
    for line in lines {
        if ended {
            return None;
        }
        if line.trim_end() == "*** End Patch" {
            ended = true;
        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
            if op.is_some() {
                return None;
            }
            op = Some(FileOp::Add {
                path: path.trim().to_owned(),
                content: String::new(),
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            if op.is_some() {
                return None;
            }
            op = Some(FileOp::Update {
                path: path.trim().to_owned(),
                old: String::new(),
                new: String::new(),
            });
        } else if line.starts_with("*** ") {
            return None;
        } else {
            match op.as_mut()? {
                FileOp::Add { content, .. } => {
                    content.push_str(line.strip_prefix('+')?);
                    content.push('\n');
                }
                FileOp::Update { .. } => {
                    if line.starts_with("@@") {
                        hunks += 1;
                    } else if let Some(text) = line.strip_prefix('-') {
                        old.push(text);
                    } else if let Some(text) = line.strip_prefix('+') {
                        new.push(text);
                    } else {
                        let text = line.strip_prefix(' ')?;
                        old.push(text);
                        new.push(text);
                    }
                }
            }
        }
    }
    if !ended {
        return None;
    }
    match op? {
        FileOp::Update { path, .. } if hunks <= 1 && !old.is_empty() => Some(FileOp::Update {
            path,
            old: old.join("\n"),
            new: new.join("\n"),
        }),
        FileOp::Update { .. } => None,
        add => Some(add),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_file_and_a_replaced_run_go_round() {
        assert_eq!(
            parse(&add("a.txt", "one\ntwo\n")),
            Some(FileOp::Add {
                path: "a.txt".into(),
                content: "one\ntwo\n".into()
            })
        );
        assert_eq!(
            parse(&update("a.txt", "two", "three")),
            Some(FileOp::Update {
                path: "a.txt".into(),
                old: "two".into(),
                new: "three".into()
            })
        );
    }

    #[test]
    fn context_lines_are_on_both_sides_and_what_is_more_than_one_change_is_none() {
        let patch = "*** Begin Patch\n*** Update File: n\n@@\n one\n-two\n+three\n*** End Patch\n";
        assert_eq!(
            parse(patch),
            Some(FileOp::Update {
                path: "n".into(),
                old: "one\ntwo".into(),
                new: "one\nthree".into()
            })
        );
        let two =
            "*** Begin Patch\n*** Update File: a\n@@\n-x\n+y\n*** Delete File: b\n*** End Patch\n";
        assert_eq!(parse(two), None);
        let hunks = "*** Begin Patch\n*** Update File: a\n@@\n-x\n+y\n@@\n-p\n+q\n*** End Patch\n";
        assert_eq!(parse(hunks), None);
        assert_eq!(parse("no patch"), None);
        assert_eq!(
            parse("*** Begin Patch\n*** Update File: a\n@@\n+only\n*** End Patch\n"),
            None
        );
    }
}
