//! Which governed steps a shell command takes, so a harness gate can hold it until green ink
//! covers it.
//!
//! The classifier reads the words of a command line the way a shell splits them and asks the
//! repository the command acts on what its refs and push configuration mean. It errs towards
//! holding: a step it cannot pin down, or a `git`/`gh` it cannot place, is an error the gate
//! turns into a refusal. Still best effort: a command that builds its arguments at run time,
//! a `gh` alias, or a program other than `git` and `gh` that pushes, is not seen. The
//! credential an approval unlocks is the guarantee; this turns attempts into requests early.

use std::collections::BTreeMap;

use super::ActionScope;

/// Where a git command runs, so its repository can be asked about refs and configuration.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitContext {
    /// Directories changed into before the command, in order (`cd DIR`, `git -C DIR`,
    /// `env -C DIR`, `sudo -D DIR`). A relative one is relative to the one before it, the
    /// first to where the command line runs.
    pub dirs: Vec<String>,
    /// The repository named by `--git-dir` or `GIT_DIR`.
    pub git_dir: Option<String>,
    /// The work tree named by `--work-tree` or `GIT_WORK_TREE`.
    pub work_tree: Option<String>,
}

/// What the classifier asks about the repository a command acts on.
pub trait Repository {
    /// The full ref `name` stands for, as `git rev-parse --symbolic-full-name NAME` prints it;
    /// `Ok(None)` when it names no ref.
    ///
    /// # Errors
    ///
    /// Why the repository could not be read there.
    fn full_name(&self, at: &GitContext, name: &str) -> Result<Option<String>, String>;

    /// Every value of the configuration `key`, in order, as `git config --get-all KEY` prints
    /// them; empty when it is unset.
    ///
    /// # Errors
    ///
    /// Why the configuration could not be read there.
    fn config(&self, at: &GitContext, key: &str) -> Result<Vec<String>, String>;
}

/// The governed steps `command` would take: each ref a `git push` updates (`push`) or removes
/// (`delete`), always with the remote it goes to, `gh pr merge` (`merge`, `PR N` or
/// `PR OWNER/REPO#N`), `gh release create` (`release`, `tag TAG`), and a `gh api` call that
/// merges a pull request. Commands inside `sh -c`, `bash -c`, `pwsh -Command`, `cmd /c`,
/// `eval`, `env -S` and behind wrappers (`sudo`, `env`, `timeout`, `nice`, `ionice`,
/// `stdbuf`, `nohup`, `setsid`, `time`, `command`, `exec`) or shell keywords are read too, up
/// to `nesting` levels deep, and a `cd` before a command moves where its repository is asked.
///
/// A `git push` without refspecs follows the repository's configuration (`remote.R.push`,
/// `push.default`, `remote.R.mirror`, `push.followTags`, the push remote of the current
/// branch), with `-c KEY=VALUE` on the command taking precedence; a git alias is expanded once.
///
/// # Errors
///
/// A sentence for the Agent when a step cannot be pinned down: a ref the repository cannot
/// name, a repository that cannot be read, a merge or release naming no pull request or tag, a
/// `gh api` call that changes refs or releases, arguments supplied by `xargs`, or a `git` or
/// `gh` the classifier cannot place in the command.
pub fn classify(
    command: &str,
    repo: &dyn Repository,
    nesting: usize,
) -> Result<Vec<ActionScope>, String> {
    let mut walk = Walk {
        repo,
        steps: Vec::new(),
        nesting,
    };
    walk.command(command, &mut Shell::default(), 0)?;
    let mut unique = Vec::new();
    for step in walk.steps {
        if !unique.contains(&step) {
            unique.push(step);
        }
    }
    Ok(unique)
}

/// What a command line has set up for the commands after it.
#[derive(Debug, Clone, Default)]
struct Shell {
    context: GitContext,
    /// `GH_REPO`, `GIT_DIR` and the like, from assignments and `export`.
    env: BTreeMap<String, String>,
}

impl Shell {
    fn assign(&mut self, name: &str, value: &str) {
        match name {
            "GIT_DIR" => self.context.git_dir = Some(value.to_owned()),
            "GIT_WORK_TREE" => self.context.work_tree = Some(value.to_owned()),
            _ => {}
        }
        self.env.insert(name.to_owned(), value.to_owned());
    }

    /// Whether configuration reaches git through the environment, which the classifier
    /// cannot read.
    fn opaque_config(&self) -> bool {
        self.env.keys().any(|name| name.starts_with("GIT_CONFIG"))
    }
}

struct Walk<'a> {
    repo: &'a dyn Repository,
    steps: Vec<ActionScope>,
    /// How deep `sh -c "sh -c ..."` and git aliases are followed.
    nesting: usize,
}

/// A simple command with its wrappers and keywords taken off.
struct Simple {
    words: Vec<String>,
    /// More arguments arrive at run time (`xargs`).
    unseen_args: bool,
    /// A command line to read instead (`env -S`).
    nested: Option<String>,
    /// The command only prints where a program is (`command -v`).
    inert: bool,
}

impl Walk<'_> {
    fn command(&mut self, command: &str, shell: &mut Shell, depth: usize) -> Result<(), String> {
        for words in split(command) {
            let mut local = shell.clone();
            let simple = peel(&words, &mut local)?;
            if simple.inert {
                continue;
            }
            if let Some(inner) = simple.nested {
                if depth < self.nesting {
                    self.command(&inner, &mut local.clone(), depth + 1)?;
                }
                continue;
            }
            let Some(program) = simple.words.first().map(|word| program_name(word)) else {
                // Only assignments: they last for the rest of the line.
                *shell = local;
                continue;
            };
            let args = &simple.words[1..];
            match program.as_str() {
                "cd" | "pushd" | "set-location" | "sl" | "chdir" => {
                    let dir = args
                        .iter()
                        .find(|arg| !arg.starts_with('-') || arg.as_str() == "-")
                        .cloned()
                        .unwrap_or_else(|| "~".to_owned());
                    shell.context.dirs.push(dir);
                }
                "export" => {
                    for arg in args {
                        if let Some((name, value)) = assignment(arg) {
                            shell.assign(name, value);
                        }
                    }
                }
                "git" => self.git(args, &local, simple.unseen_args, depth)?,
                "gh" => self.gh(args, &local, simple.unseen_args)?,
                _ => {
                    if let Some(inner) = nested(&program, args) {
                        if depth < self.nesting {
                            self.command(&inner, &mut local.clone(), depth + 1)?;
                        }
                    } else if unplaced(&simple.words) {
                        return Err(
                            "this command runs git or gh in a way the green ink gate cannot read; \
                             run the git or gh command directly"
                                .to_owned(),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn git(
        &mut self,
        args: &[String],
        shell: &Shell,
        unseen_args: bool,
        depth: usize,
    ) -> Result<(), String> {
        let mut context = shell.context.clone();
        let mut overrides = Vec::new();
        let mut opaque = shell.opaque_config();
        let mut index = 0;
        while let Some(arg) = args.get(index) {
            if !arg.starts_with('-') {
                break;
            }
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
                _ => (arg.as_str(), None),
            };
            let takes_value = matches!(
                name,
                "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
            );
            let value = match (takes_value, inline) {
                (true, Some(value)) => Some(value),
                (true, None) => {
                    index += 1;
                    args.get(index).cloned()
                }
                (false, _) => None,
            };
            match (name, value) {
                ("-C", Some(dir)) => context.dirs.push(dir),
                ("-c", Some(setting)) => overrides.push(match setting.split_once('=') {
                    Some((key, value)) => (key.to_owned(), value.to_owned()),
                    None => (setting, "true".to_owned()),
                }),
                ("--git-dir", Some(dir)) => context.git_dir = Some(dir),
                ("--work-tree", Some(dir)) => context.work_tree = Some(dir),
                ("--config-env", _) => opaque = true,
                _ => {}
            }
            index += 1;
        }
        let Some(subcommand) = args.get(index) else {
            return Ok(());
        };
        let rest = &args[index + 1..];
        let git = Git {
            repo: self.repo,
            context,
            overrides,
        };
        match subcommand.as_str() {
            "push" => {
                if unseen_args {
                    return Err(xargs_refusal());
                }
                if opaque {
                    return Err(
                        "git configuration set through --config-env or GIT_CONFIG_* cannot be \
                         read by the green ink gate; set it with -c KEY=VALUE"
                            .to_owned(),
                    );
                }
                self.push(&git, rest)
            }
            "send-pack" => Err("push with git push, which green ink can cover".to_owned()),
            known if BUILTINS.contains(&known) => Ok(()),
            alias => {
                let Some(expansion) = git.last(&format!("alias.{alias}"))? else {
                    return Ok(());
                };
                if let Some(script) = expansion.strip_prefix('!') {
                    if depth < self.nesting {
                        return self.command(
                            &format!("{script} {}", rest.join(" ")),
                            &mut shell.clone(),
                            depth + 1,
                        );
                    }
                    return Ok(());
                }
                if depth >= self.nesting {
                    return Err(format!("the git alias {alias:?} expands too deeply"));
                }
                let mut expanded = args[..index].to_vec();
                expanded.extend(split(&expansion).into_iter().flatten());
                expanded.extend(rest.iter().cloned());
                self.git(&expanded, shell, unseen_args, depth + 1)
            }
        }
    }

    fn push(&mut self, git: &Git<'_>, args: &[String]) -> Result<(), String> {
        let mut dry_run = false;
        let mut delete = false;
        let mut follow_tags = None;
        let mut patterns = Vec::new();
        let mut repo = None;
        let mut positional = Vec::new();
        let mut words = args.iter();
        while let Some(arg) = words.next() {
            match arg.as_str() {
                "--" => positional.extend(words.by_ref().cloned()),
                "--dry-run" => dry_run = true,
                "--no-dry-run" => dry_run = false,
                "--delete" => delete = true,
                "--tags" => patterns.push("refs/tags/*"),
                "--follow-tags" => follow_tags = Some(true),
                "--no-follow-tags" => follow_tags = Some(false),
                "--all" | "--branches" => patterns.push("refs/heads/*"),
                "--mirror" => patterns.push("refs/*"),
                "--repo" => repo = words.next().cloned(),
                "--push-option" | "--receive-pack" | "--exec" | "-o" => {
                    words.next();
                }
                option if option.starts_with("--repo=") => {
                    repo = Some(option["--repo=".len()..].to_owned());
                }
                option if option.starts_with("--recurse-submodules") => {
                    submodules(option.split_once('=').map(|(_, mode)| mode))?;
                }
                option if option.starts_with("--") => {}
                flags if flags.starts_with('-') && flags.len() > 1 => {
                    for (position, flag) in flags.chars().skip(1).enumerate() {
                        match flag {
                            'n' => dry_run = true,
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
        if dry_run {
            return Ok(());
        }
        submodules(git.last("push.recurseSubmodules")?.as_deref())?;
        let mut positional = positional.into_iter();
        let named_remote = repo.or_else(|| positional.next());
        let mut refspecs: Vec<String> = positional.collect();
        let mut index = 0;
        while index < refspecs.len() {
            if refspecs[index] == "tag" && index + 1 < refspecs.len() {
                let name = refspecs.remove(index + 1);
                refspecs[index] = format!("refs/tags/{name}");
            }
            index += 1;
        }
        let head = git.full_name("HEAD")?;
        let branch = head
            .as_deref()
            .and_then(|head| head.strip_prefix("refs/heads/"))
            .map(str::to_owned);
        let remote = match named_remote {
            Some(remote) => remote,
            None => git.push_remote(branch.as_deref())?,
        };
        if refspecs.is_empty() && patterns.is_empty() && !delete {
            let configured = git.all(&format!("remote.{remote}.push"))?;
            if !configured.is_empty() {
                refspecs = configured;
            } else if git.flag(&format!("remote.{remote}.mirror"))? {
                patterns.push("refs/*");
            } else {
                match git
                    .last("push.default")?
                    .unwrap_or_else(|| "simple".to_owned())
                    .as_str()
                {
                    "nothing" => {}
                    "matching" => patterns.push("refs/heads/*"),
                    "upstream" | "tracking" => {
                        let merge = branch
                            .as_deref()
                            .map(|branch| git.last(&format!("branch.{branch}.merge")))
                            .transpose()?
                            .flatten()
                            .ok_or(
                                "the current branch has no upstream to push to; name it, as in \
                                 git push origin BRANCH",
                            )?;
                        refspecs.push(format!("HEAD:{merge}"));
                    }
                    _ => refspecs.push("HEAD".to_owned()),
                }
            }
        }
        if follow_tags.map_or(git.flag("push.followTags"), Ok)? {
            patterns.push("refs/tags/*");
        }
        let scope = |kind: &str, reference: &str| {
            ActionScope::try_new(kind, format!("{remote}:{reference}"))
                .map_err(|error| error.to_string())
        };
        for pattern in patterns {
            self.steps.push(scope("push", pattern)?);
        }
        for refspec in refspecs {
            let refspec = refspec.strip_prefix('+').unwrap_or(&refspec);
            let (source, destination) = match refspec.split_once(':') {
                Some((source, destination)) => (source, destination),
                None => (refspec, refspec),
            };
            if delete || source.is_empty() {
                let reference = git.destination(destination, None, head.as_deref())?;
                self.steps.push(scope("delete", &reference)?);
            } else {
                let reference = git.destination(destination, Some(source), head.as_deref())?;
                self.steps.push(scope("push", &reference)?);
            }
        }
        Ok(())
    }

    fn gh(&mut self, args: &[String], shell: &Shell, unseen_args: bool) -> Result<(), String> {
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let mut repository = shell.env.get("GH_REPO").cloned();
        let mut method = None;
        let mut fields = false;
        let mut words = args.iter();
        while let Some(arg) = words.next() {
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
                _ => (arg.as_str(), None),
            };
            if GH_VALUE_OPTIONS.contains(&name) {
                let value = inline.or_else(|| words.next().cloned());
                match name {
                    "-R" | "--repo" => repository = value,
                    "-X" | "--method" => method = value.map(|method| method.to_ascii_uppercase()),
                    "-f" | "-F" | "--field" | "--raw-field" | "--input" => fields = true,
                    _ => {}
                }
            } else if arg.starts_with('-') {
                flags.push(arg.as_str());
            } else {
                positional.push(arg.as_str());
            }
        }
        let governed = matches!(
            positional.as_slice(),
            ["pr", "merge", ..] | ["release", "create", ..] | ["api", ..]
        );
        if governed && unseen_args {
            return Err(xargs_refusal());
        }
        match positional.as_slice() {
            ["pr", "merge", rest @ ..] => {
                if flags.contains(&"--disable-auto") {
                    return Ok(());
                }
                let (url_repository, number) =
                    rest.first().and_then(|which| pull_request(which)).ok_or(
                        "name the pull request by number, as in gh pr merge 42, so green ink can \
                         cover it",
                    )?;
                self.merge(url_repository.or(repository), &number)?;
            }
            ["release", "create", rest @ ..] => {
                let tag = rest
                    .first()
                    .ok_or("name the release's tag, as in gh release create v1.2.0")?;
                self.steps.push(
                    ActionScope::try_new("release", format!("tag {tag}"))
                        .map_err(|e| e.to_string())?,
                );
            }
            ["api", path, ..] => {
                let method =
                    method.unwrap_or_else(|| (if fields { "POST" } else { "GET" }).to_owned());
                if matches!(method.as_str(), "GET" | "HEAD") {
                    return Ok(());
                }
                self.api(path.trim_start_matches('/'), &method, args)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn merge(&mut self, repository: Option<String>, number: &str) -> Result<(), String> {
        let target = match repository {
            Some(repository) => format!("PR {repository}#{number}"),
            None => format!("PR {number}"),
        };
        self.steps
            .push(ActionScope::try_new("merge", target).map_err(|e| e.to_string())?);
        Ok(())
    }

    /// A `gh api` call that changes something: merging a pull request is a merge; one that
    /// moves refs, tags, releases or a repository's contents is held, because green ink covers
    /// those steps through `git push` and `gh release create`.
    fn api(&mut self, path: &str, method: &str, args: &[String]) -> Result<(), String> {
        let segments: Vec<&str> = path
            .split(['?', '#'])
            .next()
            .unwrap_or(path)
            .split('/')
            .collect();
        if let ["repos", owner, name, "pulls", number, "merge"] = segments.as_slice() {
            let repository = (!owner.starts_with('{') && !name.starts_with('{'))
                .then(|| format!("{owner}/{name}"));
            return self.merge(repository, number);
        }
        let changes_refs =
            segments.iter().any(|segment| {
                matches!(
                    *segment,
                    "releases" | "refs" | "tags" | "merges" | "contents" | "branches"
                )
            }) || (method == "DELETE" && segments.len() <= 3 && segments.first() == Some(&"repos"))
                || (path == "graphql" && args.iter().any(|arg| arg.contains("mutation")));
        if changes_refs {
            return Err(format!(
                "this gh api call ({method} {path}) changes refs, releases or a repository, which \
                 green ink covers only through git push, gh pr merge and gh release create"
            ));
        }
        Ok(())
    }
}

/// Builtin git commands, which a git alias of the same name cannot replace.
const BUILTINS: &[&str] = &[
    "add",
    "am",
    "apply",
    "bisect",
    "blame",
    "branch",
    "checkout",
    "cherry-pick",
    "clean",
    "clone",
    "commit",
    "config",
    "describe",
    "diff",
    "fetch",
    "format-patch",
    "grep",
    "help",
    "init",
    "log",
    "ls-files",
    "ls-remote",
    "merge",
    "mv",
    "notes",
    "pull",
    "rebase",
    "reflog",
    "remote",
    "reset",
    "restore",
    "rev-parse",
    "revert",
    "rm",
    "show",
    "stash",
    "status",
    "submodule",
    "switch",
    "tag",
];

/// Refuses a push that also pushes submodules, which may go anywhere.
fn submodules(mode: Option<&str>) -> Result<(), String> {
    match mode {
        Some("on-demand" | "only") => Err(
            "a push that also pushes submodules cannot be covered by green ink; push each \
             repository directly"
                .to_owned(),
        ),
        _ => Ok(()),
    }
}

fn xargs_refusal() -> String {
    "the arguments of this command come from xargs, which the green ink gate cannot see; run \
     it directly"
        .to_owned()
}

/// A repository and the `-c` settings a git command line carries.
struct Git<'a> {
    repo: &'a dyn Repository,
    context: GitContext,
    overrides: Vec<(String, String)>,
}

impl Git<'_> {
    fn full_name(&self, name: &str) -> Result<Option<String>, String> {
        self.repo
            .full_name(&self.context, name)
            .map_err(|error| format!("the repository could not be read: {error}"))
    }

    /// Every value of `key`: the repository's, then the command line's.
    fn all(&self, key: &str) -> Result<Vec<String>, String> {
        let mut values = self.repo.config(&self.context, key).map_err(|error| {
            format!("the repository's configuration could not be read: {error}")
        })?;
        values.extend(
            self.overrides
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.clone()),
        );
        Ok(values)
    }

    fn last(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.all(key)?.pop())
    }

    fn flag(&self, key: &str) -> Result<bool, String> {
        Ok(self.last(key)?.is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1" | ""
            )
        }))
    }

    /// The remote a push without one goes to, as git chooses it.
    fn push_remote(&self, branch: Option<&str>) -> Result<String, String> {
        if let Some(branch) = branch
            && let Some(remote) = self.last(&format!("branch.{branch}.pushRemote"))?
        {
            return Ok(remote);
        }
        if let Some(remote) = self.last("remote.pushDefault")? {
            return Ok(remote);
        }
        if let Some(branch) = branch
            && let Some(remote) = self.last(&format!("branch.{branch}.remote"))?
        {
            return Ok(remote);
        }
        Ok("origin".to_owned())
    }

    /// The full ref a refspec's destination updates. `heads/`, `tags/` and `remotes/` are
    /// under `refs/`; a bare name takes the namespace of its source (a tag pushed under another
    /// name is still a tag), else what the repository says it is.
    fn destination(
        &self,
        name: &str,
        source: Option<&str>,
        head: Option<&str>,
    ) -> Result<String, String> {
        if name.starts_with("refs/") {
            return Ok(name.to_owned());
        }
        if name == "HEAD" || name == "@" {
            return head
                .map(str::to_owned)
                .filter(|head| head.starts_with("refs/"))
                .ok_or_else(|| {
                    "the current branch could not be named; push it by name, as in git push \
                     origin BRANCH"
                        .to_owned()
                });
        }
        if ["heads/", "tags/", "remotes/"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            return Ok(format!("refs/{name}"));
        }
        if name.contains(['*', '?', '[']) {
            return Ok(format!("refs/heads/{name}"));
        }
        let namespace = |reference: &str| {
            ["refs/heads/", "refs/tags/"]
                .into_iter()
                .find(|prefix| reference.starts_with(prefix))
        };
        if let Some(source) = source.filter(|source| *source != name) {
            let source = if source == "HEAD" || source == "@" {
                head.map(str::to_owned)
            } else {
                self.full_name(source)?
            };
            if let Some(prefix) = source.as_deref().and_then(namespace) {
                return Ok(format!("{prefix}{name}"));
            }
        }
        self.full_name(name)?
            .filter(|reference| reference.starts_with("refs/"))
            .ok_or_else(|| {
                format!(
                    "the green ink gate cannot tell which ref {name:?} is; name it in full, as \
                     refs/heads/{name} or refs/tags/{name}"
                )
            })
    }
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

/// Shell keywords a command may follow; `time` is read as a wrapper.
const KEYWORDS: &[&str] = &[
    "if", "then", "elif", "else", "while", "until", "do", "{", "!",
];

/// A wrapper that runs the rest of its words as a command, and how it reads its options.
struct Wrapper {
    /// Short options that take a value, attached or as the next word.
    short_values: &'static str,
    /// Long options that take a value, `--name=value` or as the next word.
    long_values: &'static [&'static str],
    /// Operands before the command (`timeout DURATION`).
    operands: usize,
}

fn wrapper(program: &str) -> Option<Wrapper> {
    let (short_values, long_values, operands): (&str, &[&str], usize) = match program {
        "sudo" | "doas" => (
            "CDghprRtTUu",
            &[
                "--close-from",
                "--chdir",
                "--group",
                "--host",
                "--prompt",
                "--chroot",
                "--role",
                "--type",
                "--command-timeout",
                "--other-user",
                "--user",
            ],
            0,
        ),
        "env" => ("uCS", &["--unset", "--chdir", "--split-string"], 0),
        "timeout" => ("sk", &["--signal", "--kill-after"], 1),
        "nice" => ("n", &["--adjustment"], 0),
        "ionice" => (
            "cnpPu",
            &["--class", "--classdata", "--pid", "--pgid", "--uid"],
            0,
        ),
        "stdbuf" => ("ioe", &["--input", "--output", "--error"], 0),
        "xargs" => (
            "adEILnPs",
            &[
                "--arg-file",
                "--delimiter",
                "--eof",
                "--replace",
                "--max-lines",
                "--max-args",
                "--max-procs",
                "--max-chars",
                "--process-slot-var",
            ],
            0,
        ),
        "command" | "nohup" | "setsid" | "time" | "builtin" => ("", &[], 0),
        "exec" => ("a", &[], 0),
        _ => return None,
    };
    Some(Wrapper {
        short_values,
        long_values,
        operands,
    })
}

/// Takes assignments, keywords and wrappers off the front of a simple command, recording
/// what they change in `shell`.
fn peel(words: &[String], shell: &mut Shell) -> Result<Simple, String> {
    let mut simple = Simple {
        words: Vec::new(),
        unseen_args: false,
        nested: None,
        inert: false,
    };
    let mut rest = words;
    while let Some(first) = rest.first() {
        if let Some((name, value)) = assignment(first) {
            shell.assign(name, value);
            rest = &rest[1..];
            continue;
        }
        if KEYWORDS.contains(&first.as_str()) {
            rest = &rest[1..];
            continue;
        }
        let program = program_name(first);
        let Some(spec) = wrapper(&program) else {
            break;
        };
        rest = &rest[1..];
        if program == "xargs" {
            simple.unseen_args = true;
        }
        // `nice -10 git ...` gives the adjustment as a bare number.
        while let Some(word) = rest.first() {
            if word == "--" {
                rest = &rest[1..];
                break;
            }
            if program == "env" && assignment(word).is_some() {
                let (name, value) = assignment(word).unwrap_or_default();
                shell.assign(name, value);
                rest = &rest[1..];
                continue;
            }
            if !word.starts_with('-') || word.len() == 1 {
                break;
            }
            rest = &rest[1..];
            let (option, value) = if let Some(long) = word.strip_prefix("--") {
                let (name, inline) = match long.split_once('=') {
                    Some((name, value)) => (name, Some(value.to_owned())),
                    None => (long, None),
                };
                let name = format!("--{name}");
                let value = if spec.long_values.contains(&name.as_str()) {
                    match inline {
                        Some(value) => Some(value),
                        None => {
                            let value = rest.first().cloned();
                            rest = rest.get(1..).unwrap_or_default();
                            value
                        }
                    }
                } else {
                    None
                };
                (name, value)
            } else {
                let mut value = None;
                let mut option = String::new();
                for (position, flag) in word.char_indices().skip(1) {
                    if program == "command" && matches!(flag, 'v' | 'V') {
                        simple.inert = true;
                    }
                    if spec.short_values.contains(flag) {
                        option = format!("-{flag}");
                        let attached = &word[position + flag.len_utf8()..];
                        value = if attached.is_empty() {
                            let next = rest.first().cloned();
                            rest = rest.get(1..).unwrap_or_default();
                            next
                        } else {
                            Some(attached.to_owned())
                        };
                        break;
                    }
                }
                (option, value)
            };
            match (program.as_str(), option.as_str(), value) {
                ("env", "-C" | "--chdir", Some(dir))
                | ("sudo" | "doas", "-D" | "--chdir", Some(dir)) => {
                    shell.context.dirs.push(dir);
                }
                ("env", "-S" | "--split-string", Some(line)) => {
                    let mut line = line;
                    for word in rest {
                        line.push(' ');
                        line.push_str(word);
                    }
                    simple.nested = Some(line);
                    return Ok(simple);
                }
                _ => {}
            }
        }
        for _ in 0..spec.operands {
            rest = rest.get(1..).unwrap_or_default();
        }
    }
    simple.words = rest.to_vec();
    Ok(simple)
}

/// `NAME=value` as a shell assignment.
fn assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !name.as_bytes()[0].is_ascii_digit())
    .then_some((name, value))
}

/// A program's name without its directory or `.exe`, in lower case.
fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let base = base.to_ascii_lowercase();
    base.strip_suffix(".exe")
        .map_or(base.clone(), str::to_owned)
}

/// Whether a command that is neither `git` nor `gh` still names one with a governed
/// subcommand after it (`find . -exec git push ...`, `Start-Process git -ArgumentList push`).
fn unplaced(words: &[String]) -> bool {
    words.iter().enumerate().any(|(index, word)| {
        let later = &words[index + 1..];
        match program_name(word).as_str() {
            "git" => later.iter().any(|word| word.contains("push")),
            "gh" => later
                .iter()
                .any(|word| matches!(word.as_str(), "merge" | "release" | "api")),
            _ => false,
        }
    })
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

/// `gh` options, global or of the subcommands read here, that take the next word as their
/// value.
const GH_VALUE_OPTIONS: &[&str] = &[
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
    "-X",
    "--method",
    "-f",
    "--field",
    "--raw-field",
    "-H",
    "--header",
    "--input",
    "-q",
    "--jq",
    "--template",
    "--hostname",
    "-p",
    "--preview",
    "--cache",
];

/// A pull request given as `42`, `#42` or a `.../OWNER/REPO/pull/42` URL: the repository the
/// URL names, and the number.
fn pull_request(which: &str) -> Option<(Option<String>, String)> {
    let (repository, number) = match which.split_once("/pull/") {
        Some((before, rest)) => {
            let mut parts = before.rsplit('/');
            let name = parts.next().filter(|name| !name.is_empty());
            let owner = parts.next().filter(|owner| !owner.is_empty());
            let repository = owner
                .zip(name)
                .map(|(owner, name)| format!("{owner}/{name}"));
            (
                repository,
                rest.split(['/', '#', '?']).next().unwrap_or(rest),
            )
        }
        None => (None, which),
    };
    let number = number.trim_start_matches('#');
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| (repository, number.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::super::{GreenInkPolicy, Requirement};
    use super::*;

    /// A repository on branch `main` (tracking `origin`) with a tag `v1.2.0`, a branch
    /// `ai6/mission-7`, and the configuration given. Each question is recorded with where it
    /// was asked.
    #[derive(Default)]
    struct Fake {
        config: Vec<(&'static str, &'static str)>,
        detached: bool,
        unreadable: bool,
        asked: RefCell<Vec<GitContext>>,
    }

    impl Repository for Fake {
        fn full_name(&self, at: &GitContext, name: &str) -> Result<Option<String>, String> {
            self.asked.borrow_mut().push(at.clone());
            if self.unreadable {
                return Err("not a git repository".to_owned());
            }
            Ok(match name {
                "HEAD" if self.detached => None,
                "HEAD" => Some("refs/heads/main".to_owned()),
                "v1.2.0" => Some("refs/tags/v1.2.0".to_owned()),
                "main" | "ai6/mission-7" | "feature" => Some(format!("refs/heads/{name}")),
                _ => None,
            })
        }

        fn config(&self, at: &GitContext, key: &str) -> Result<Vec<String>, String> {
            self.asked.borrow_mut().push(at.clone());
            if self.unreadable {
                return Err("not a git repository".to_owned());
            }
            Ok(self
                .config
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| (*value).to_owned())
                .collect())
        }
    }

    fn with(config: &[(&'static str, &'static str)], command: &str) -> Vec<(String, String)> {
        let repo = Fake {
            config: config.to_vec(),
            ..Fake::default()
        };
        classify(command, &repo, 4)
            .unwrap_or_else(|error| panic!("{command}: {error}"))
            .into_iter()
            .map(|scope| (scope.kind, scope.target))
            .collect()
    }

    fn steps(command: &str) -> Vec<(String, String)> {
        with(&[], command)
    }

    fn held(command: &str) -> String {
        classify(command, &Fake::default(), 4)
            .err()
            .unwrap_or_else(|| panic!("{command} should be held"))
    }

    fn one(kind: &str, target: &str) -> Vec<(String, String)> {
        vec![(kind.to_owned(), target.to_owned())]
    }

    fn pushes(targets: &[&str]) -> Vec<(String, String)> {
        targets
            .iter()
            .map(|target| ("push".to_owned(), (*target).to_owned()))
            .collect()
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
            ("git push", "origin:refs/heads/main", true),
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
        assert!(steps("apt-get install -y git gh").is_empty());
    }

    #[test]
    fn wrappers_and_keywords_do_not_hide_a_push() {
        for command in [
            "timeout 60 git push origin main",
            "timeout -s KILL -k 5 60 git push origin main",
            "timeout --signal=TERM 1m git push origin main",
            "nice -n 10 git push origin main",
            "nice -10 git push origin main",
            "ionice -c 3 git push origin main",
            "stdbuf -oL git push origin main",
            "env -C . git push origin main",
            "env -i PATH=/usr/bin git push origin main",
            "env -S 'git push origin' main",
            "sudo -u builder git push origin main",
            "sudo --user=builder -D /work git push origin main",
            "command git push origin main",
            "exec -a pusher git push origin main",
            "nohup git push origin main",
            "setsid -f git push origin main",
            "time -p git push origin main",
            "if true; then git push origin main; fi",
            "for b in main; do git push origin main; done",
            "while false; do :; done; until git push origin main; do :; done",
            "{ git push origin main; }",
            "! git push origin main",
            "if ! timeout 60 nice -n 5 git push origin main; then echo no; fi",
        ] {
            assert_eq!(
                steps(command),
                one("push", "origin:refs/heads/main"),
                "{command}"
            );
        }
        assert!(
            steps("command -v git").is_empty(),
            "command -v runs nothing"
        );
    }

    #[test]
    fn a_git_or_gh_the_classifier_cannot_place_is_held() {
        for command in [
            "find . -name x -exec git push origin main \\;",
            "parallel git push origin ::: main",
            "Start-Process git -ArgumentList 'push origin main'",
            "echo main | xargs git push origin",
            "echo 42 | xargs gh pr merge",
        ] {
            held(command);
        }
    }

    #[test]
    fn a_destination_with_a_pattern_is_protected_wherever_its_star_is() {
        let policy = GreenInkPolicy::default();
        let found = steps("git push origin 'refs/heads/*ain:refs/heads/*ain'");
        assert_eq!(found, one("push", "origin:refs/heads/*ain"));
        for target in [
            "origin:refs/heads/*ain",
            "origin:refs/heads/ma?n",
            "origin:refs/heads/[m]ain",
        ] {
            let scope = ActionScope::try_new("push", target).unwrap();
            assert_eq!(
                policy.requirement("field", &scope),
                Requirement::GreenInk,
                "{target}"
            );
        }
    }

    #[test]
    fn short_ref_namespaces_are_read_as_full_refs_and_an_unknown_name_is_held() {
        assert_eq!(
            steps("git push origin HEAD:heads/main"),
            one("push", "origin:refs/heads/main")
        );
        assert_eq!(
            steps("git push origin HEAD:tags/v9"),
            one("push", "origin:refs/tags/v9")
        );
        assert_eq!(
            steps("git push origin v1.2.0:release-tag"),
            one("push", "origin:refs/tags/release-tag"),
            "a tag pushed under another name is a tag"
        );
        let error = held("git push origin topic");
        assert!(error.contains("refs/heads/topic"), "{error}");
        held("git push origin abc123:newbranch");
    }

    #[test]
    fn a_push_without_refspecs_follows_the_repositorys_configuration() {
        assert_eq!(
            with(
                &[("remote.origin.push", "refs/heads/*:refs/heads/*")],
                "git push"
            ),
            one("push", "origin:refs/heads/*")
        );
        assert_eq!(
            with(
                &[("remote.origin.push", "HEAD:refs/heads/release/9")],
                "git push origin"
            ),
            one("push", "origin:refs/heads/release/9")
        );
        assert_eq!(
            with(
                &[
                    ("push.default", "upstream"),
                    ("branch.main.merge", "refs/heads/trunk")
                ],
                "git push"
            ),
            one("push", "origin:refs/heads/trunk")
        );
        assert_eq!(
            steps("git -c push.default=upstream -c branch.main.merge=refs/heads/trunk push"),
            one("push", "origin:refs/heads/trunk"),
            "-c on the command line counts"
        );
        assert_eq!(
            with(&[("push.default", "matching")], "git push"),
            one("push", "origin:refs/heads/*")
        );
        assert!(with(&[("push.default", "nothing")], "git push").is_empty());
        assert_eq!(
            with(&[("remote.backup.mirror", "true")], "git push backup"),
            one("push", "backup:refs/*")
        );
        assert_eq!(
            with(&[("push.followTags", "true")], "git push origin main"),
            pushes(&["origin:refs/tags/*", "origin:refs/heads/main"])
        );
        assert_eq!(
            steps("git push --follow-tags origin main"),
            pushes(&["origin:refs/tags/*", "origin:refs/heads/main"])
        );
        assert_eq!(
            with(
                &[("push.followTags", "true")],
                "git push --no-follow-tags origin main"
            ),
            one("push", "origin:refs/heads/main")
        );
        assert_eq!(
            steps("git push --tags origin main"),
            pushes(&["origin:refs/tags/*", "origin:refs/heads/main"]),
            "--tags with a refspec pushes both"
        );
        assert_eq!(
            steps("git push --dry-run origin main --no-dry-run"),
            one("push", "origin:refs/heads/main")
        );
        held("git -c push.recurseSubmodules=on-demand push origin main");
        held("GIT_CONFIG_COUNT=1 git push origin main");
        held("git --config-env=push.default=X push");
    }

    #[test]
    fn a_plain_push_goes_to_the_remote_git_would_choose() {
        assert_eq!(
            with(&[("branch.main.remote", "upstream")], "git push"),
            one("push", "upstream:refs/heads/main")
        );
        assert_eq!(
            with(
                &[
                    ("branch.main.remote", "upstream"),
                    ("remote.pushDefault", "fork")
                ],
                "git push"
            ),
            one("push", "fork:refs/heads/main")
        );
        assert_eq!(
            with(
                &[
                    ("remote.pushDefault", "fork"),
                    ("branch.main.pushRemote", "mine")
                ],
                "git push"
            ),
            one("push", "mine:refs/heads/main")
        );
        let detached = Fake {
            detached: true,
            ..Fake::default()
        };
        assert!(classify("git push", &detached, 4).is_err());
    }

    #[test]
    fn the_repository_is_asked_where_the_command_runs() {
        let repo = Fake::default();
        classify(
            "cd /work/a && GIT_DIR=/srv/repo.git git -C sub push origin main",
            &repo,
            4,
        )
        .unwrap();
        let asked = repo.asked.borrow();
        assert!(!asked.is_empty());
        for context in asked.iter() {
            assert_eq!(context.dirs, ["/work/a", "sub"]);
            assert_eq!(context.git_dir.as_deref(), Some("/srv/repo.git"));
        }
        drop(asked);

        let repo = Fake::default();
        classify(
            "export GIT_WORK_TREE=/w; git --git-dir=/g push origin main",
            &repo,
            4,
        )
        .unwrap();
        for context in repo.asked.borrow().iter() {
            assert_eq!(context.git_dir.as_deref(), Some("/g"));
            assert_eq!(context.work_tree.as_deref(), Some("/w"));
        }

        let unreadable = Fake {
            unreadable: true,
            ..Fake::default()
        };
        let error = classify("cd elsewhere && git push origin main", &unreadable, 4).unwrap_err();
        assert!(error.contains("could not be read"), "{error}");
    }

    #[test]
    fn a_git_alias_is_expanded_once() {
        assert_eq!(
            with(&[("alias.ship", "push origin")], "git ship main"),
            one("push", "origin:refs/heads/main")
        );
        assert_eq!(
            steps("git -c alias.p=push p origin main"),
            one("push", "origin:refs/heads/main")
        );
        assert_eq!(
            with(&[("alias.go", "!git push origin")], "git go main"),
            one("push", "origin:refs/heads/main")
        );
        assert!(steps("git status").is_empty());
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
            pushes(&["origin:refs/heads/main", "origin:refs/tags/v1.2.0"])
        );
    }

    #[test]
    fn merges_and_releases_name_the_pull_request_and_the_tag() {
        for command in [
            "gh pr merge 42 --squash",
            "gh pr merge '#42' -t 'Squashed title'",
        ] {
            assert_eq!(steps(command), one("merge", "PR 42"), "{command}");
        }
        for command in [
            "gh pr merge https://github.com/o/r/pull/42 --auto",
            "gh -R o/r pr merge 42",
            "gh pr merge 42 --repo=o/r",
            "GH_REPO=o/r gh pr merge 42",
            "gh api -X PUT repos/o/r/pulls/42/merge",
            "gh api repos/o/r/pulls/42/merge -f merge_method=squash",
        ] {
            assert_eq!(steps(command), one("merge", "PR o/r#42"), "{command}");
        }
        assert_eq!(
            steps("gh api --method put repos/{owner}/{repo}/pulls/42/merge"),
            one("merge", "PR 42")
        );
        assert!(steps("gh pr merge 42 --disable-auto").is_empty());
        assert_eq!(
            steps("gh release create v1.2.0 --notes 'first' dist/app.tar.gz"),
            one("release", "tag v1.2.0")
        );
        assert!(steps("gh pr view 42").is_empty());
        assert!(steps("gh release list").is_empty());
        assert!(steps("gh api repos/o/r/pulls/42").is_empty(), "a read");
    }

    #[test]
    fn a_gh_api_call_that_moves_refs_or_releases_is_held() {
        for command in [
            "gh api -X POST repos/o/r/releases -f tag_name=v2",
            "gh api --method PATCH repos/o/r/git/refs/heads/main -f sha=abc",
            "gh api -X DELETE repos/o/r/git/refs/heads/main",
            "gh api repos/o/r/git/refs -f ref=refs/heads/main -f sha=abc",
            "gh api -X PUT repos/o/r/contents/README.md -f message=x",
            "gh api -X POST repos/o/r/merges -f base=main -f head=x",
            "gh api -X DELETE repos/o/r",
            "gh api graphql -f query='mutation { mergePullRequest(input: {}) { clientMutationId } }'",
        ] {
            held(command);
        }
    }

    #[test]
    fn a_step_that_cannot_be_named_is_an_error_for_the_agent() {
        let detached = Fake {
            detached: true,
            ..Fake::default()
        };
        let error = classify("git push origin HEAD", &detached, 4).unwrap_err();
        assert!(error.contains("git push origin BRANCH"), "{error}");
        held("gh pr merge --squash");
        held("gh release create");
    }
}
