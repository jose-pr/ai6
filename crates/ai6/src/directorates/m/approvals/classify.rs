//! Which governed steps a shell command takes, so a harness gate can hold it until green ink
//! covers it.
//!
//! Best effort by design: the classifier reads the words of a command line the way a shell
//! would split them, so a command that builds its arguments at run time, or reaches git
//! another way, is not seen. The credential an approval unlocks is what makes the step need
//! green ink; this only turns attempts into requests early.

use super::ActionScope;

/// The governed steps `command` would take: `git push` (each ref it updates, or `delete` for
/// each ref it removes), `gh pr merge` and `gh release create`. Commands inside `sh -c`,
/// `bash -c`, `pwsh -Command`, `cmd /c` and `eval` are read too; a `--dry-run` push takes no
/// step.
///
/// `resolve` names the full ref for a ref as written (`HEAD`, `main`, `v1.2.0`), as
/// `git rev-parse --symbolic-full-name` would in the command's repository; `None` when it
/// cannot tell. A bare name it cannot resolve is taken as a branch.
///
/// # Errors
///
/// A sentence for the Agent when a governed step cannot be pinned down: a push of the current
/// branch whose name `resolve` does not know, or a merge or release that names no pull request
/// or tag.
pub fn classify(
    command: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<ActionScope>, String> {
    let mut steps = Vec::new();
    collect(command, resolve, &mut steps, 0)?;
    let mut unique = Vec::new();
    for step in steps {
        if !unique.contains(&step) {
            unique.push(step);
        }
    }
    Ok(unique)
}

/// How deep `sh -c "sh -c ..."` is followed.
const MAX_NESTING: usize = 4;

fn collect(
    command: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    steps: &mut Vec<ActionScope>,
    depth: usize,
) -> Result<(), String> {
    for words in split(command) {
        let words = strip_wrappers(&words);
        let Some(program) = words.first().map(|word| program_name(word)) else {
            continue;
        };
        let args = &words[1..];
        if let Some(inner) = nested(&program, args) {
            if depth < MAX_NESTING {
                collect(&inner, resolve, steps, depth + 1)?;
            }
            continue;
        }
        match program.as_str() {
            "git" => git(args, resolve, steps)?,
            "gh" => gh(args, steps)?,
            _ => {}
        }
    }
    Ok(())
}

/// Splits a command line into simple commands of words, honouring quotes. Separators are
/// `;`, `&`, `|`, newlines, parentheses and backticks outside quotes.
fn split(command: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars().peekable();
    let end_word = |word: &mut String, words: &mut Vec<String>, in_word: &mut bool| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\' | '$' | '`')) => {
                            word.extend(chars.next());
                        }
                        _ => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some('\n') | None => {}
                    Some(next) => word.push(next),
                }
            }
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' => {
                end_word(&mut word, &mut words, &mut in_word);
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => end_word(&mut word, &mut words, &mut in_word),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    end_word(&mut word, &mut words, &mut in_word);
    if !words.is_empty() {
        commands.push(words);
    }
    commands
}

/// Drops leading `NAME=value` assignments and the wrappers that run the rest as a command.
fn strip_wrappers(words: &[String]) -> &[String] {
    let mut rest = words;
    loop {
        match rest.first() {
            Some(word) if is_assignment(word) => rest = &rest[1..],
            Some(word)
                if matches!(
                    program_name(word).as_str(),
                    "sudo" | "env" | "command" | "exec" | "nohup" | "time" | "nice"
                ) =>
            {
                rest = &rest[1..];
                while rest
                    .first()
                    .is_some_and(|word| word.starts_with('-') || is_assignment(word))
                {
                    rest = &rest[1..];
                }
            }
            _ => return rest,
        }
    }
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && !name.as_bytes()[0].is_ascii_digit()
    })
}

/// A program's name without its directory or `.exe`, in lower case.
fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let base = base.to_ascii_lowercase();
    base.strip_suffix(".exe")
        .map_or(base.clone(), str::to_owned)
}

/// The command a shell or `eval` runs from its arguments.
fn nested(program: &str, args: &[String]) -> Option<String> {
    let after = |flags: &[&str]| {
        let position = args
            .iter()
            .position(|arg| flags.iter().any(|flag| arg.eq_ignore_ascii_case(flag)))?;
        Some(args[position + 1..].join(" "))
    };
    match program {
        "sh" | "bash" | "zsh" | "dash" | "ksh" => {
            let position = args.iter().position(|arg| {
                arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c')
            })?;
            args.get(position + 1).cloned()
        }
        "pwsh" | "powershell" => after(&["-c", "-command"]),
        "cmd" => after(&["/c", "/k"]),
        "eval" => Some(args.join(" ")),
        _ => None,
    }
}

/// Global options of `git` that take the next word as their value.
const GIT_VALUE_OPTIONS: [&str; 6] = [
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
];

fn git(
    args: &[String],
    resolve: &dyn Fn(&str) -> Option<String>,
    steps: &mut Vec<ActionScope>,
) -> Result<(), String> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if !arg.starts_with('-') {
            break;
        }
        index += if GIT_VALUE_OPTIONS.contains(&arg.as_str()) {
            2
        } else {
            1
        };
    }
    if args.get(index).map(String::as_str) == Some("push") {
        push(&args[index + 1..], resolve, steps)?;
    }
    Ok(())
}

/// `git push` options that take the next word as their value.
const PUSH_VALUE_OPTIONS: [&str; 5] = ["--repo", "--push-option", "--receive-pack", "--exec", "-o"];

fn push(
    args: &[String],
    resolve: &dyn Fn(&str) -> Option<String>,
    steps: &mut Vec<ActionScope>,
) -> Result<(), String> {
    let mut delete = false;
    let mut everything = None;
    let mut repo = None;
    let mut positional = Vec::new();
    let mut words = args.iter();
    while let Some(arg) = words.next() {
        match arg.as_str() {
            "--" => positional.extend(words.by_ref().cloned()),
            "--dry-run" => return Ok(()),
            "--delete" => delete = true,
            "--tags" => everything = Some("refs/tags/*"),
            "--all" | "--branches" => everything = Some("refs/heads/*"),
            "--mirror" => everything = Some("refs/*"),
            option if PUSH_VALUE_OPTIONS.contains(&option) => {
                let value = words.next();
                if option == "--repo" {
                    repo = value.cloned();
                }
            }
            option if option.starts_with("--repo=") => {
                repo = Some(option["--repo=".len()..].to_owned());
            }
            option if option.starts_with("--") => {}
            flags if flags.starts_with('-') && flags.len() > 1 => {
                for (position, flag) in flags.chars().skip(1).enumerate() {
                    match flag {
                        'n' => return Ok(()),
                        'd' => delete = true,
                        // `-o` takes a value: the rest of the word, or the next word.
                        'o' => {
                            if position + 2 == flags.len() {
                                words.next();
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
            _ => positional.push(arg.clone()),
        }
    }
    let mut positional = positional.into_iter();
    let remote = repo.or_else(|| positional.next());
    let mut refspecs: Vec<String> = positional.collect();
    // `git push REMOTE tag NAME` is `refs/tags/NAME`.
    let mut index = 0;
    while index < refspecs.len() {
        if refspecs[index] == "tag" && index + 1 < refspecs.len() {
            let name = refspecs.remove(index + 1);
            refspecs[index] = format!("refs/tags/{name}");
        }
        index += 1;
    }
    let scope = |kind: &str, reference: &str| {
        let target = match &remote {
            Some(remote) => format!("{remote}:{reference}"),
            None => reference.to_owned(),
        };
        ActionScope::try_new(kind, target).map_err(|error| error.to_string())
    };
    if let Some(pattern) = everything {
        steps.push(scope("push", pattern)?);
        return Ok(());
    }
    if refspecs.is_empty() {
        if delete {
            return Ok(());
        }
        refspecs.push("HEAD".to_owned());
    }
    for refspec in refspecs {
        let refspec = refspec.strip_prefix('+').unwrap_or(&refspec);
        let (source, destination) = match refspec.split_once(':') {
            Some((source, destination)) => (source, destination),
            None => (refspec, refspec),
        };
        if delete || source.is_empty() {
            let reference = full_ref(destination, None, resolve)?;
            steps.push(scope("delete", &reference)?);
        } else {
            let reference = full_ref(destination, Some(source), resolve)?;
            steps.push(scope("push", &reference)?);
        }
    }
    Ok(())
}

/// The full ref `name` stands for. A bare destination takes the namespace of its `source`
/// (a tag pushed under another name is still a tag), else of what `resolve` says it is,
/// else is a branch.
fn full_ref(
    name: &str,
    source: Option<&str>,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    if name.starts_with("refs/") {
        return Ok(name.to_owned());
    }
    if name == "HEAD" || name == "@" {
        return resolve("HEAD")
            .filter(|reference| reference.starts_with("refs/"))
            .ok_or_else(|| {
                "the current branch could not be named; push it by name, as in \
                 git push origin BRANCH"
                    .to_owned()
            });
    }
    let source = source.filter(|source| *source != name);
    let namespace = |reference: &str| {
        ["refs/heads/", "refs/tags/"]
            .into_iter()
            .find(|prefix| reference.starts_with(prefix))
    };
    if let Some(prefix) = source
        .and_then(|source| full_ref(source, None, resolve).ok())
        .as_deref()
        .and_then(namespace)
    {
        return Ok(format!("{prefix}{name}"));
    }
    Ok(resolve(name)
        .filter(|reference| reference.starts_with("refs/"))
        .unwrap_or_else(|| format!("refs/heads/{name}")))
}

/// `gh` options, global or of the subcommands read here, that take the next word as their
/// value.
const GH_VALUE_OPTIONS: [&str; 18] = [
    "-R",
    "--repo",
    "-t",
    "--subject",
    "--title",
    "-b",
    "--body",
    "-F",
    "--body-file",
    "--notes-file",
    "-A",
    "--author-email",
    "--match-head-commit",
    "-n",
    "--notes",
    "--target",
    "--discussion-category",
    "--notes-start-tag",
];

fn gh(args: &[String], steps: &mut Vec<ActionScope>) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut words = args.iter();
    while let Some(arg) = words.next() {
        if GH_VALUE_OPTIONS.contains(&arg.as_str()) {
            words.next();
        } else if arg.starts_with('-') {
            flags.push(arg.as_str());
        } else {
            positional.push(arg.as_str());
        }
    }
    match positional.as_slice() {
        ["pr", "merge", rest @ ..] => {
            if flags.contains(&"--disable-auto") {
                return Ok(());
            }
            let number = rest.first().and_then(|which| pull_request(which)).ok_or(
                "name the pull request by number, as in gh pr merge 42, so green ink can cover it",
            )?;
            steps.push(
                ActionScope::try_new("merge", format!("PR {number}")).map_err(|e| e.to_string())?,
            );
        }
        ["release", "create", rest @ ..] => {
            let tag = rest
                .first()
                .ok_or("name the release's tag, as in gh release create v1.2.0")?;
            steps.push(
                ActionScope::try_new("release", format!("tag {tag}")).map_err(|e| e.to_string())?,
            );
        }
        _ => {}
    }
    Ok(())
}

/// The number of a pull request given as `42`, `#42` or a `.../pull/42` URL.
fn pull_request(which: &str) -> Option<String> {
    let number = which
        .split_once("/pull/")
        .map_or(which, |(_, rest)| {
            rest.split(['/', '#', '?']).next().unwrap_or(rest)
        })
        .trim_start_matches('#');
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| number.to_owned())
}

#[cfg(test)]
mod tests {
    use super::super::{GreenInkPolicy, Requirement};
    use super::*;

    /// A repository on branch `main` with a tag `v1.2.0` and a branch `ai6/mission-7`.
    fn repository(name: &str) -> Option<String> {
        match name {
            "HEAD" => Some("refs/heads/main".to_owned()),
            "v1.2.0" => Some("refs/tags/v1.2.0".to_owned()),
            "main" | "ai6/mission-7" => Some(format!("refs/heads/{name}")),
            _ => None,
        }
    }

    fn steps(command: &str) -> Vec<(String, String)> {
        classify(command, &repository)
            .unwrap_or_else(|error| panic!("{command}: {error}"))
            .into_iter()
            .map(|scope| (scope.kind, scope.target))
            .collect()
    }

    fn one(kind: &str, target: &str) -> Vec<(String, String)> {
        vec![(kind.to_owned(), target.to_owned())]
    }

    #[test]
    fn a_push_names_each_ref_it_updates_and_the_policy_protects_only_some() {
        let policy = GreenInkPolicy::default();
        for (command, target, protected) in [
            ("git push origin main", "origin:refs/heads/main", true),
            (
                "git push origin ai6/mission-7",
                "origin:refs/heads/ai6/mission-7",
                false,
            ),
            (
                "git push origin HEAD:refs/heads/main",
                "origin:refs/heads/main",
                true,
            ),
            (
                "git push origin HEAD:release/2.0",
                "origin:refs/heads/release/2.0",
                true,
            ),
            ("git push origin v1.2.0", "origin:refs/tags/v1.2.0", true),
            (
                "git push origin tag v1.2.0",
                "origin:refs/tags/v1.2.0",
                true,
            ),
            (
                "git push upstream feature",
                "upstream:refs/heads/feature",
                false,
            ),
            ("git push", "refs/heads/main", true),
            ("git push -u origin HEAD", "origin:refs/heads/main", true),
        ] {
            let found = steps(command);
            assert_eq!(found, one("push", target), "{command}");
            let scope = ActionScope::try_new("push", target).unwrap();
            let needs = policy.requirement("field", &scope) == Requirement::GreenInk;
            assert_eq!(needs, protected, "{command}");
        }
    }

    #[test]
    fn force_and_other_flags_do_not_hide_the_ref() {
        for command in [
            "git push --force origin main",
            "git push -f origin main",
            "git push origin +main",
            "git push --force-with-lease=main:abc123 origin main",
            "git push origin main --force",
            "git -C /work/repo -c push.default=simple push origin main",
            "git --no-pager push -o ci.skip origin main",
            "git push --repo=origin main",
            "/usr/bin/git push origin main",
            "git.exe push origin main",
        ] {
            assert_eq!(
                steps(command),
                one("push", "origin:refs/heads/main"),
                "{command}"
            );
        }
    }

    #[test]
    fn quoting_separators_and_shells_are_read_like_a_shell() {
        for command in [
            "git push 'origin' \"main\"",
            "cd /work && git push origin main",
            "cargo test; git push origin main || echo failed",
            "GIT_TRACE=1 git push origin main",
            "sudo -E git push origin main",
            "bash -lc 'git push origin main'",
            "sh -c \"cd /work && git push origin \\\"main\\\"\"",
            "pwsh -NoProfile -Command git push origin main",
            "cmd /c git push origin main",
            "eval git push origin main",
            "(git push origin main)",
            "git push origin ma\\\nin",
        ] {
            assert_eq!(
                steps(command),
                one("push", "origin:refs/heads/main"),
                "{command}"
            );
        }
        assert!(steps("echo 'git push origin main'").is_empty());
        assert!(steps("git log --grep push").is_empty());
        assert!(steps("git commit -m 'push origin main'").is_empty());
    }

    #[test]
    fn deletions_wildcards_and_dry_runs() {
        assert_eq!(
            steps("git push origin --delete ai6/mission-7"),
            one("delete", "origin:refs/heads/ai6/mission-7")
        );
        assert_eq!(
            steps("git push -d origin v1.2.0"),
            one("delete", "origin:refs/tags/v1.2.0")
        );
        assert_eq!(
            steps("git push origin :main"),
            one("delete", "origin:refs/heads/main")
        );
        assert_eq!(
            steps("git push --tags origin"),
            one("push", "origin:refs/tags/*")
        );
        assert_eq!(
            steps("git push --all origin"),
            one("push", "origin:refs/heads/*")
        );
        assert_eq!(
            steps("git push --mirror backup"),
            one("push", "backup:refs/*")
        );
        assert!(steps("git push --dry-run origin main").is_empty());
        assert!(steps("git push -n origin main").is_empty());

        let policy = GreenInkPolicy::default();
        for wildcard in ["origin:refs/heads/*", "origin:refs/tags/*", "backup:refs/*"] {
            let scope = ActionScope::try_new("push", wildcard).unwrap();
            assert_eq!(
                policy.requirement("field", &scope),
                Requirement::GreenInk,
                "{wildcard} reaches protected refs"
            );
        }
    }

    #[test]
    fn several_pushes_in_one_line_are_each_named_once() {
        assert_eq!(
            steps("git push origin main v1.2.0 && git push origin main"),
            vec![
                ("push".to_owned(), "origin:refs/heads/main".to_owned()),
                ("push".to_owned(), "origin:refs/tags/v1.2.0".to_owned()),
            ]
        );
    }

    #[test]
    fn merges_and_releases_name_the_pull_request_and_the_tag() {
        for command in [
            "gh pr merge 42 --squash",
            "gh pr merge '#42' -t 'Squashed title'",
            "gh pr merge https://github.com/o/r/pull/42 --auto",
            "gh -R o/r pr merge 42",
        ] {
            assert_eq!(steps(command), one("merge", "PR 42"), "{command}");
        }
        assert!(steps("gh pr merge 42 --disable-auto").is_empty());
        assert_eq!(
            steps("gh release create v1.2.0 --notes 'first' dist/app.tar.gz"),
            one("release", "tag v1.2.0")
        );
        assert!(steps("gh pr view 42").is_empty());
        assert!(steps("gh release list").is_empty());
    }

    #[test]
    fn a_step_that_cannot_be_named_is_an_error_for_the_agent() {
        let unknown = |_: &str| None;
        let error = classify("git push", &unknown).unwrap_err();
        assert!(error.contains("git push origin BRANCH"), "{error}");
        assert!(classify("gh pr merge --squash", &unknown).is_err());
        assert!(classify("gh release create", &unknown).is_err());
        // An unresolved name is a branch.
        assert_eq!(
            classify("git push origin topic", &unknown).unwrap()[0].target,
            "origin:refs/heads/topic"
        );
    }
}
