use std::process::Command;

use super::testing::{open_tree, overlay_dir, pin_of, release_store};
use super::*;
use crate::directorates::q::overlays::{OverlayFrom, OverlayRecord};

/// A skill's whole text, as the checks want it.
const SKILL: &str = "---\nname: review\ndescription: Reviews a change.\n---\n\nHow to review.\n";

fn no_secret(name: &str) -> Result<zeroize::Zeroizing<String>, String> {
    Err(format!("no secret {name}"))
}

fn record(name: &str, dir: &Path) -> OverlayRecord {
    OverlayRecord::new(name, OverlayFrom::Path(dir.to_path_buf())).pinned(pin_of(dir))
}

fn records<const N: usize>(list: [OverlayRecord; N]) -> BTreeMap<String, OverlayRecord> {
    list.into_iter()
        .map(|record| (record.identity(), record))
        .collect()
}

struct Case {
    state: tempfile::TempDir,
    limits: OverlayLimits,
}

impl Case {
    fn new() -> Self {
        Self {
            state: tempfile::tempdir().unwrap(),
            limits: OverlayLimits::default(),
        }
    }

    fn prepare(
        &self,
        loadout: &[&str],
        operation: &[&str],
        records: &BTreeMap<String, OverlayRecord>,
    ) -> Result<Option<Prepared>, String> {
        let owned = |names: &[&str]| -> Vec<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };
        prepare(
            &Request {
                loadout: &owned(loadout),
                operation: &owned(operation),
                records,
                limits: &self.limits,
                state_dir: self.state.path(),
                base: None,
            },
            &no_secret,
        )
    }

    fn deliver(
        &self,
        prepared: &Prepared,
        environment: &[(&str, &str)],
        commands: bool,
        handler: &str,
    ) -> Result<Delivered, String> {
        let work = self.state.path().join("work");
        fs::create_dir_all(&work).unwrap();
        self.deliver_in(
            prepared,
            environment,
            commands,
            handler,
            &work.to_string_lossy(),
        )
    }
}

impl Case {
    fn deliver_in(
        &self,
        prepared: &Prepared,
        environment: &[(&str, &str)],
        commands: bool,
        handler: &str,
        working_directory: &str,
    ) -> Result<Delivered, String> {
        let environment: BTreeMap<String, String> = environment
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        let keeper = Keeper::default();
        let fit = prepared.fit(
            Site::Here {
                dir: self.state.path().join("overlays"),
            },
            &environment,
            commands,
            true,
            (&keeper, Duration::from_secs(60)),
        )?;
        prepared.deliver(
            &fit,
            SessionFacts {
                environment,
                handler,
                working_directory,
                key: "mission-1",
            },
            &keeper,
        )
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        release_store(self.state.path());
    }
}

fn path_separator() -> char {
    if cfg!(windows) { ';' } else { ':' }
}

#[test]
fn an_overlay_is_stored_by_its_digest_and_needs_no_second_copy() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("kb/a.md", "a\n")]);
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let digests = prepared.digests();
    assert_eq!(digests.len(), 1);
    assert_eq!(digests[0].digest, pin_of(&dir).to_string());
    let stored = case
        .state
        .path()
        .join("overlays/trees")
        .join(hex_of(&pin_of(&dir)));
    assert!(stored.join("kb/a.md").is_file());
    // Its source can go: the store holds its pin.
    fs::remove_dir_all(&dir).unwrap();
    assert!(case.prepare(&["tools"], &[], &list).unwrap().is_some());
}

#[test]
fn nothing_issued_is_no_delivery() {
    let case = Case::new();
    assert!(case.prepare(&[], &[], &BTreeMap::new()).unwrap().is_none());
    assert!(!case.state.path().join("overlays").exists());
}

#[test]
fn a_source_with_other_bytes_than_its_pin_refuses_and_nothing_overrides_it() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("kb/a.md", "a\n")]);
    let list = records([record("tools", &dir)]);
    fs::write(dir.join("kb/a.md"), "changed\n").unwrap();
    let refusal = case.prepare(&["tools"], &[], &list).err().unwrap();
    assert!(refusal.contains("not its pin"), "{refusal}");
    assert!(
        !case.state.path().join("overlays/trees").exists() || {
            fs::read_dir(case.state.path().join("overlays/trees"))
                .unwrap()
                .next()
                .is_none()
        }
    );
}

#[test]
fn a_tampered_store_refuses_the_start_and_is_not_fetched_over() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("kb/a.md", "a\n")]);
    let list = records([record("tools", &dir)]);
    case.prepare(&["tools"], &[], &list).unwrap();
    let stored = case
        .state
        .path()
        .join("overlays/trees")
        .join(hex_of(&pin_of(&dir)));
    // As a Session with the Station's own OS user could, it takes the protection off first.
    open_tree(&stored);
    fs::write(stored.join("kb/a.md"), "planted\n").unwrap();
    let refusal = case.prepare(&["tools"], &[], &list).err().unwrap();
    assert!(
        refusal.contains("changed after it was fetched"),
        "{refusal}"
    );
    assert!(refusal.contains(&hex_of(&pin_of(&dir))), "{refusal}");
    assert_eq!(
        fs::read_to_string(stored.join("kb/a.md")).unwrap(),
        "planted\n"
    );
    // An added file is a change too.
    fs::write(stored.join("kb/a.md"), "a\n").unwrap();
    assert!(case.prepare(&["tools"], &[], &list).is_ok());
    open_tree(&stored);
    fs::write(stored.join("kb/b.md"), "added\n").unwrap();
    assert!(case.prepare(&["tools"], &[], &list).is_err());
}

#[test]
fn a_copy_in_the_store_refuses_what_a_session_tries_to_write_there() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[("bin/helper", "#!/bin/sh\necho hi\n")],
    );
    let list = records([record("tools", &dir)]);
    case.prepare(&["tools"], &[], &list).unwrap();
    let stored = case
        .state
        .path()
        .join("overlays/trees")
        .join(hex_of(&pin_of(&dir)));
    assert!(
        fs::write(stored.join("bin/helper"), "planted\n").is_err(),
        "a file of the overlay was written"
    );
    assert!(
        fs::write(stored.join("planted.txt"), "planted\n").is_err(),
        "a new file went in the overlay's root"
    );
    assert!(
        fs::write(stored.join("bin/planted"), "planted\n").is_err(),
        "a new file went in a directory of the overlay"
    );
    // An elevated Windows token deletes through a deny entry (measured: writing it stays refused,
    // and a limited token cannot delete); the next start's check is what catches that.
    if !crate::station::elevation::is_elevated().unwrap_or(false) {
        assert!(
            fs::remove_file(stored.join("bin/helper")).is_err(),
            "a file of the overlay was deleted"
        );
        assert_eq!(pin_of(&stored), pin_of(&dir));
    }
}

#[cfg(unix)]
#[test]
fn a_program_stays_executable_in_the_store_and_one_from_a_copy_with_no_modes_becomes_so() {
    use std::os::unix::fs::PermissionsExt;
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[("bin/helper", "#!/bin/sh\necho hi\n"), ("kb/a.md", "a\n")],
    );
    let list = records([record("tools", &dir)]);
    case.prepare(&["tools"], &[], &list).unwrap();
    let stored = case
        .state
        .path()
        .join("overlays/trees")
        .join(hex_of(&pin_of(&dir)));
    let mode = |path: &str| {
        fs::metadata(stored.join(path))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode("bin/helper"), 0o555);
    assert_eq!(mode("kb/a.md"), 0o444);
}

#[test]
fn a_session_gets_the_root_bin_on_path_and_a_desk_one_no_bin() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "ship-tools",
        &[
            ("bin/helper", "#!/bin/sh\necho hi\n"),
            ("env", "SHIP_MODE=fast\n"),
        ],
    );
    let list = records([record("ship-tools", &dir)]);
    let prepared = case.prepare(&["ship-tools"], &[], &list).unwrap().unwrap();
    let base = format!("/agent/bin{}/usr/bin", path_separator());

    let field = case
        .deliver(&prepared, &[("PATH", base.as_str())], true, "codex")
        .unwrap();
    let root = field.environment["SHIP_TOOLS_OVERLAY_ROOT"].clone();
    assert!(root.ends_with(&hex_of(&pin_of(&dir))), "{root}");
    let first = field.environment["PATH"]
        .split(path_separator())
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(Path::new(&first), Path::new(&root).join("bin"));
    assert!(field.environment["PATH"].ends_with(&base));
    assert_eq!(field.environment["SHIP_MODE"], "fast");
    assert_eq!(field.overlays.roots, std::slice::from_ref(&root));

    let desk = case
        .deliver(&prepared, &[("PATH", base.as_str())], false, "codex")
        .unwrap();
    assert_eq!(
        desk.environment["PATH"], base,
        "a desk Session gets no bin/"
    );
    assert_eq!(desk.environment["SHIP_TOOLS_OVERLAY_ROOT"], root);
    assert!(desk.context.unwrap().contains("ship-tools"));
}

#[test]
fn an_agent_with_no_path_is_still_found_where_it_was() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("bin/helper", "x\n")]);
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let field = case.deliver(&prepared, &[], true, "codex").unwrap();
    let path = &field.environment["PATH"];
    assert!(path.contains("bin"));
    if !cfg!(windows) {
        assert!(path.ends_with("/bin:/usr/bin"), "{path}");
    }
    let desk = case.deliver(&prepared, &[], false, "codex").unwrap();
    assert!(
        !desk.environment.contains_key("PATH"),
        "{:?}",
        desk.environment
    );
}

#[test]
fn an_overlay_may_not_set_what_is_the_stations() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[("env", "AI6_GREEN_INK_GATE=/tmp/mine\n")],
    );
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let refusal = case.deliver(&prepared, &[], true, "codex").err().unwrap();
    assert!(refusal.contains("AI6_GREEN_INK_GATE"), "{refusal}");
}

#[test]
fn rules_routing_and_context_arrive_in_composition_order_each_marked() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let first = overlay_dir(
        source.path(),
        "first",
        &[
            (
                "overlay.cfg",
                "routing = [\"- Rust work: read kb/rust.md\"]\npriority = 1\n",
            ),
            ("rules/style.md", "Style rule body\n"),
            ("CONTEXT.md", "---\ntitle: dropped\n---\nContext body\n"),
            ("kb/rust.md", "not in the context\n"),
        ],
    );
    let second = overlay_dir(
        source.path(),
        "second",
        &[
            ("overlay.cfg", "priority = 2\n"),
            ("rules/later.md", "Later rule\n"),
        ],
    );
    let list = records([record("first", &first), record("second", &second)]);
    // The Operation's list comes second, and composition order is by priority.
    let prepared = case
        .prepare(&["second"], &["first"], &list)
        .unwrap()
        .unwrap();
    let text = case
        .deliver(&prepared, &[], true, "codex")
        .unwrap()
        .context
        .unwrap();
    for wanted in [
        "- Rust work: read kb/rust.md",
        "From `rules/style.md`:\nStyle rule body",
        "From `CONTEXT.md`:\nContext body",
        "From `rules/later.md`:\nLater rule",
        "$FIRST_OVERLAY_ROOT",
    ] {
        assert!(text.contains(wanted), "{wanted:?} in {text}");
    }
    assert!(!text.contains("title: dropped"), "{text}");
    assert!(!text.contains("not in the context"), "{text}");
    assert!(text.find("Style rule body") < text.find("Later rule"));
}

#[test]
fn context_past_its_setting_is_refused_by_the_settings_name() {
    let mut case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[("CONTEXT.md", "long enough text\n")],
    );
    let list = records([record("tools", &dir)]);
    case.limits.context_bytes = crate::bounds::Limit::at(16);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let refusal = case.deliver(&prepared, &[], true, "codex").err().unwrap();
    assert!(
        refusal.contains("station.overlays.context_bytes"),
        "{refusal}"
    );
}

#[test]
fn skills_are_laid_out_as_a_plugin_and_a_hint_makes_one_a_routing_line() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let native = overlay_dir(
        source.path(),
        "native",
        &[
            ("skills/review/SKILL.md", SKILL),
            ("skills/review/refs/more.md", "more\n"),
        ],
    );
    let hinted = overlay_dir(
        source.path(),
        "hinted",
        &[
            ("overlay.cfg", "[harness.claude-code]\nskills = false\n"),
            (
                "skills/audit/SKILL.md",
                "---\nname: audit\ndescription: Audits.\n---\nBody\n",
            ),
        ],
    );
    let list = records([record("native", &native), record("hinted", &hinted)]);
    let prepared = case
        .prepare(&["native", "hinted"], &[], &list)
        .unwrap()
        .unwrap();
    let delivered = case.deliver(&prepared, &[], true, "claude").unwrap();
    let skills = &delivered.overlays.skills;
    assert_eq!(skills.len(), 2);
    let audit = skills.iter().find(|skill| skill.name == "audit").unwrap();
    let review = skills.iter().find(|skill| skill.name == "review").unwrap();
    assert!(!audit.native, "the hint turns native delivery off");
    assert!(review.native);
    assert_eq!(review.description, "Reviews a change.");
    assert_eq!(review.overlay, "native");
    let plugin = PathBuf::from(delivered.overlays.plugin_dir.clone().unwrap());
    assert!(plugin.join(".claude-plugin/plugin.json").is_file());
    assert!(plugin.join("skills/review/SKILL.md").is_file());
    assert!(plugin.join("skills/review/refs/more.md").is_file());
    assert!(!plugin.join("skills/audit").exists(), "not a native skill");
    assert!(
        fs::write(plugin.join("skills/review/SKILL.md"), "planted").is_err(),
        "the plugin is as read-only as the overlays"
    );
    let routing = delivered.overlays.routing(true).unwrap();
    assert!(
        routing.contains("audit") && !routing.contains("`review`"),
        "{routing}"
    );
    // Another handler is not bound by a hint for Claude Code.
    let codex = case.deliver(&prepared, &[], true, "codex").unwrap();
    assert!(codex.overlays.skills.iter().all(|skill| skill.native));
}

#[test]
fn a_changed_plugin_refuses_as_a_changed_overlay_does() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("skills/review/SKILL.md", SKILL)]);
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let delivered = case.deliver(&prepared, &[], true, "claude").unwrap();
    let plugin = PathBuf::from(delivered.overlays.plugin_dir.unwrap());
    open_tree(&plugin);
    fs::write(plugin.join("skills/review/SKILL.md"), "planted").unwrap();
    let refusal = case.deliver(&prepared, &[], true, "claude").err().unwrap();
    assert!(refusal.contains("changed after it was made"), "{refusal}");
}

#[test]
fn a_skill_of_the_same_name_is_the_later_overlays_and_the_collision_is_reported() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let early = overlay_dir(
        source.path(),
        "early",
        &[
            ("overlay.cfg", "priority = 1\n"),
            ("skills/review/SKILL.md", SKILL),
        ],
    );
    let late = overlay_dir(
        source.path(),
        "late",
        &[
            ("overlay.cfg", "priority = 2\n"),
            ("skills/review/SKILL.md", SKILL),
        ],
    );
    let list = records([record("early", &early), record("late", &late)]);
    let prepared = case
        .prepare(&["early", "late"], &[], &list)
        .unwrap()
        .unwrap();
    let delivered = case.deliver(&prepared, &[], true, "codex").unwrap();
    assert_eq!(delivered.overlays.skills.len(), 1);
    assert_eq!(delivered.overlays.skills[0].overlay, "late");
    assert!(
        delivered
            .weakenings
            .iter()
            .any(|reported| reported.weakening.id == "skill-collision"
                && reported.weakening.subject.contains("early")),
        "{:?}",
        delivered.weakenings
    );
}

#[test]
fn a_skill_that_fails_its_check_refuses_the_overlay() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[(
            "skills/bad/SKILL.md",
            "---\nname: bad\ndescription: Grants.\nallowed-tools: Bash\n---\nBody\n",
        )],
    );
    let list = records([record("tools", &dir)]);
    let refusal = case.prepare(&["tools"], &[], &list).err().unwrap();
    assert!(refusal.contains("skill \"bad\""), "{refusal}");
}

#[test]
fn requires_brings_what_it_names_and_a_missing_one_refuses() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let base = overlay_dir(source.path(), "base", &[("kb/a.md", "a\n")]);
    let top = overlay_dir(
        source.path(),
        "top",
        &[("overlay.cfg", "requires = [\"base\"]\n")],
    );
    let both = records([record("base", &base), record("top", &top)]);
    let prepared = case.prepare(&["top"], &[], &both).unwrap().unwrap();
    let names: Vec<String> = prepared.digests().into_iter().map(|d| d.overlay).collect();
    assert_eq!(names, ["base", "top"]);
    let alone = records([record("top", &top)]);
    let refusal = case.prepare(&["top"], &[], &alone).err().unwrap();
    assert!(refusal.contains("requires \"base\""), "{refusal}");
}

#[test]
fn an_overlay_with_no_pin_is_refused_unless_allowed_and_is_reported_when_it_is() {
    let mut case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("kb/a.md", "a\n")]);
    let list = records([OverlayRecord::new("tools", OverlayFrom::Path(dir.clone()))]);
    let refusal = case.prepare(&["tools"], &[], &list).err().unwrap();
    assert!(refusal.contains("no pin"), "{refusal}");
    case.limits.unpinned = true;
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let delivered = case.deliver(&prepared, &[], true, "codex").unwrap();
    assert!(
        delivered
            .weakenings
            .iter()
            .any(|reported| reported.weakening.id == "unpinned" && reported.overlay == "tools"),
        "{:?}",
        delivered.weakenings
    );
}

#[test]
fn a_store_inside_the_working_directory_is_refused() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("kb/a.md", "a\n")]);
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let refusal = case
        .deliver_in(
            &prepared,
            &[],
            true,
            "codex",
            &case.state.path().to_string_lossy(),
        )
        .err()
        .unwrap();
    assert!(
        refusal.contains("where a commit could hold it"),
        "{refusal}"
    );
}

fn git(dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .arg("-C")
        .arg(dir)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository holding `overlays/tools/` with a file whose bytes a checkout filter would change.
fn repository(root: &Path) -> (PathBuf, String) {
    let repo = root.join("agents.git");
    fs::create_dir_all(repo.join("overlays/tools/kb")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join(".gitattributes"), "*.md text eol=crlf\n").unwrap();
    fs::write(repo.join("overlays/tools/kb/a.md"), "line\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    (repo, commit)
}

#[test]
fn a_git_source_is_stored_as_its_blobs_with_no_checkout_filter() {
    let case = Case::new();
    let root = tempfile::tempdir().unwrap();
    let (repo, commit) = repository(root.path());
    // What `git cat-file blob` prints: LF, whatever a checkout would write.
    let published = overlay_dir(root.path(), "published", &[("kb/a.md", "line\n")]);
    let text = format!("{}@{commit}#overlays/tools", repo.to_string_lossy());
    let source = Source::parse(&format!("git+{text}")).unwrap();
    assert_eq!(source.kind, SourceKind::Git);
    let by_commit =
        OverlayRecord::new("tools", OverlayFrom::Source(source)).pinned(pin_of(&published));
    let prepared = case
        .prepare(&["tools"], &[], &records([by_commit]))
        .unwrap()
        .unwrap();
    assert_eq!(prepared.digests()[0].digest, pin_of(&published).to_string());
    let scratch = case.state.path().join("overlays/scratch");
    assert_eq!(fs::read_dir(scratch).unwrap().count(), 0, "nothing is left");
}

#[test]
fn a_moved_branch_is_a_pin_mismatch() {
    let case = Case::new();
    let root = tempfile::tempdir().unwrap();
    let (repo, _) = repository(root.path());
    let published = overlay_dir(root.path(), "published", &[("kb/a.md", "line\n")]);
    fs::write(repo.join("overlays/tools/kb/b.md"), "later\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "two"]);
    let source = Source::parse(&format!(
        "git+{}@main#overlays/tools",
        repo.to_string_lossy()
    ))
    .unwrap();
    let moved = OverlayRecord::new("tools", OverlayFrom::Source(source)).pinned(pin_of(&published));
    let refusal = case
        .prepare(&["tools"], &[], &records([moved]))
        .err()
        .unwrap();
    assert!(refusal.contains("not its pin"), "{refusal}");
}

#[test]
fn a_link_in_a_git_tree_is_refused() {
    let case = Case::new();
    let root = tempfile::tempdir().unwrap();
    let (repo, _) = repository(root.path());
    // A link by its tree entry, which holds on a platform that cannot make one.
    let object = git(&repo, &["rev-parse", "HEAD:overlays/tools/kb/a.md"]);
    git(
        &repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{object},overlays/tools/kb/link"),
        ],
    );
    git(&repo, &["commit", "-q", "-m", "link"]);
    let source = Source::parse(&format!(
        "git+{}@main#overlays/tools",
        repo.to_string_lossy()
    ))
    .unwrap();
    let limits = OverlayLimits {
        unpinned: true,
        ..OverlayLimits::default()
    };
    let linked = OverlayRecord::new("tools", OverlayFrom::Source(source));
    let mut case = case;
    case.limits = limits;
    let refusal = case
        .prepare(&["tools"], &[], &records([linked]))
        .err()
        .unwrap();
    assert!(refusal.contains("symlink: kb/link"), "{refusal}");
}

#[test]
fn a_plain_url_source_is_refused_in_words() {
    let case = Case::new();
    let source = Source::parse("https://example.org/overlays/rust").unwrap();
    let plain = OverlayRecord::new("rust", OverlayFrom::Source(source)).pinned(Digest::of(b"x"));
    let refusal = case
        .prepare(&["rust"], &[], &records([plain]))
        .err()
        .unwrap();
    assert!(refusal.contains("plain URL"), "{refusal}");
}

#[test]
fn a_change_between_digests_is_named_and_none_is_none() {
    let issued = |overlay: &str, digest: &str| IssuedDigest {
        overlay: overlay.to_owned(),
        digest: digest.to_owned(),
    };
    let before = [issued("a", "sha256:1"), issued("b", "sha256:2")];
    assert_eq!(change_between(&before, &before), None);
    let after = [issued("a", "sha256:9"), issued("c", "sha256:3")];
    let change = change_between(&before, &after).unwrap();
    assert!(change.contains("a: sha256:1 -> sha256:9"), "{change}");
    assert!(change.contains("b: sha256:2 removed"), "{change}");
    assert!(change.contains("c: added at sha256:3"), "{change}");
}

#[test]
fn front_matter_is_only_a_closed_block_at_the_start() {
    assert_eq!(without_front_matter("---\na: b\n---\nbody"), "body");
    assert_eq!(without_front_matter("---\r\na: b\r\n...\r\nbody"), "body");
    assert_eq!(without_front_matter("\u{feff}---\nx\n---\nbody"), "body");
    assert_eq!(without_front_matter("---\nno close"), "---\nno close");
    assert_eq!(
        without_front_matter("body\n---\nx\n---\n"),
        "body\n---\nx\n---\n"
    );
}

/// A `PATH` holding the host's Python, or `None` where it has none that runs.
fn path_with_python() -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    let found = Site::Here {
        dir: PathBuf::from("."),
    }
    .find_tools(&["python3", "python"], &path)
    .ok()?;
    found
        .values()
        .any(|tool| tool.version.contains("Python 3"))
        .then_some(path)
}

/// What a Session's own process would print, run with exactly its environment.
fn session_prints(environment: &BTreeMap<String, String>, program: &str, args: &[&str]) -> String {
    let mut command = Command::new(program);
    command.args(args).env_clear().envs(environment);
    #[cfg(windows)]
    command.env(
        "SYSTEMROOT",
        std::env::var_os("SYSTEMROOT").unwrap_or_default(),
    );
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn on_this_host_python_imports_the_overlays_lib_and_setup_runs_once_per_digest() {
    let Some(path) = path_with_python() else {
        eprintln!("skipped: this host has no Python 3 on its PATH");
        return;
    };
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[
            ("lib/python/shiphelp.py", "VALUE = 'from the lib'\n"),
            (
                "setup.py",
                "import os, sys\n\
                 with open(os.path.join(os.environ['AOS_OVERLAY_STATE'], 'phases'), 'a') as out:\n\
                 \x20   out.write(sys.argv[1] + ' ' + os.environ['AOS_SETUP_PHASE'] + '\\n')\n",
            ),
            (
                "overlay.cfg",
                "[[setup]]\ninterpreter = \"python\"\npath = \"setup.py\"\n",
            ),
        ],
    );
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    // Windows' Python launcher reads these, as an Agent's env would give them.
    let profile: Vec<(&str, String)> = ["LOCALAPPDATA", "APPDATA", "USERPROFILE"]
        .into_iter()
        .filter_map(|name| Some((name, std::env::var(name).ok()?)))
        .collect();
    let mut environment = vec![("PATH", path.as_str())];
    environment.extend(profile.iter().map(|(name, value)| (*name, value.as_str())));
    let environment = environment.as_slice();
    let delivered = case.deliver(&prepared, environment, true, "codex").unwrap();
    let python = delivered.environment["AGENTS_PYTHON"].clone();
    assert_eq!(
        session_prints(
            &delivered.environment,
            &python,
            &["-c", "import shiphelp; print(shiphelp.VALUE)"]
        ),
        "from the lib"
    );
    let state = delivered.environment["TOOLS_OVERLAY_STATE"].clone();
    let phases = || fs::read_to_string(Path::new(&state).join("phases")).unwrap();
    assert_eq!(phases().trim(), "install install");
    assert!(
        fs::write(Path::new(&state).join("planted"), "x").is_err(),
        "the state directory is read-only to the Session"
    );
    let again = case.deliver(&prepared, environment, true, "codex").unwrap();
    assert_eq!(phases().trim(), "install install", "{:?}", again.digests);
}

#[test]
fn on_this_host_code_with_no_python_on_the_path_refuses_naming_what_is_missing() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("lib/python/m.py", "x = 1\n")]);
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let nowhere = source.path().join("nothing-here");
    fs::create_dir_all(&nowhere).unwrap();
    let nowhere = nowhere.to_string_lossy().into_owned();
    let refusal = case
        .deliver(&prepared, &[("PATH", nowhere.as_str())], true, "codex")
        .err()
        .unwrap();
    assert!(refusal.contains("python3 or python"), "{refusal}");
    assert!(refusal.contains("tools"), "{refusal}");
    // Text goes anywhere, and a desk Session asks nothing of the host.
    let desk = case.deliver(&prepared, &[("PATH", nowhere.as_str())], false, "codex");
    assert!(desk.is_ok(), "{:?}", desk.err());
}

#[test]
fn commands_are_refused_without_the_setting_and_a_wrong_pin_refuses_with_it() {
    let Some(path) = path_with_python() else {
        eprintln!("skipped: this host has no Python 3 on its PATH");
        return;
    };
    let mut case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(source.path(), "tools", &[("cmds/hello.py", "print(1)\n")]);
    let list = records([record("tools", &dir)]);
    let environment = [("PATH", path.as_str())];
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let refusal = case
        .deliver(&prepared, &environment, true, "codex")
        .err()
        .unwrap();
    assert!(
        refusal.contains("station.overlays.dotagents_pyz"),
        "{refusal}"
    );

    let pyz = source.path().join("dotagents.pyz");
    fs::write(&pyz, "print('stand-in')\n").unwrap();
    case.limits.dotagents_pyz = Some(pyz.to_string_lossy().into_owned());
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let refusal = case
        .deliver(&prepared, &environment, true, "codex")
        .err()
        .unwrap();
    assert!(
        refusal.contains("station.overlays.dotagents_pin"),
        "{refusal}"
    );

    case.limits.dotagents_pin = Some(Digest::of(b"other").to_string());
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let refusal = case
        .deliver(&prepared, &environment, true, "codex")
        .err()
        .unwrap();
    assert!(
        refusal.contains("not station.overlays.dotagents_pin"),
        "{refusal}"
    );

    case.limits.dotagents_pin = Some(Digest::of(b"print('stand-in')\n").to_string());
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let delivered = case
        .deliver(&prepared, &environment, true, "codex")
        .unwrap();
    assert!(delivered.environment["AGENTS_CMDS_PATH"].contains("cmds"));
    let home = &delivered.environment["AGENTS_HOME"];
    assert_eq!(
        fs::read_dir(home).unwrap().count(),
        0,
        "its agents directory is empty"
    );
    assert!(
        delivered
            .environment
            .keys()
            .all(|name| !name.starts_with("DOTAGENTS_")),
        "{:?}",
        delivered.environment.keys()
    );
    let site = Site::Here {
        dir: case.state.path().join("overlays"),
    };
    forget_session(&site, "mission-1").unwrap();
    assert!(!Path::new(home).exists(), "removed with its Mission");
}

/// A container reached through the real provider, when the environment names one: Podman up, a
/// running container, and where the provider's library is built.
#[test]
fn in_a_container_a_helper_reaches_its_commands_and_code_with_no_python_waits() {
    let (Ok(container), Ok(plugins)) = (
        std::env::var("AI6_TEST_OVERLAY_CONTAINER"),
        std::env::var("AI6_TEST_OVERLAY_PLUGINS"),
    ) else {
        eprintln!("skipped: AI6_TEST_OVERLAY_CONTAINER and AI6_TEST_OVERLAY_PLUGINS are not set");
        return;
    };
    use crate::directorates::m::theaters::TheaterAccess;
    let access = TheaterAccess::Transport(
        crate::TransportAccess::new("container")
            .with_setting("container", container.as_str())
            .with_setting(
                "engine",
                std::env::var("AI6_TEST_ENGINE").unwrap_or_else(|_| "podman".to_owned()),
            ),
    );
    let loaded = crate::station::plugins::Plugins::scan(&[PathBuf::from(plugins)]);
    let client = Arc::new(loaded.theater_transport(&access).unwrap());

    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[
            ("bin/ship-helper", "#!/bin/sh\necho HELPER-IN-CONTAINER\n"),
            ("rules/a.md", "A rule.\n"),
        ],
    );
    let list = records([record("tools", &dir)]);
    let prepared = case.prepare(&["tools"], &[], &list).unwrap().unwrap();
    let keeper = Keeper::default();
    let environment = BTreeMap::new();
    let fit = prepared
        .fit(
            Site::theater("box", client.clone()).unwrap(),
            &environment,
            true,
            true,
            (&keeper, Duration::from_secs(60)),
        )
        .unwrap();
    let delivered = prepared
        .deliver(
            &fit,
            SessionFacts {
                environment: environment.clone(),
                handler: "codex",
                working_directory: "/work",
                key: "mission-1",
            },
            &keeper,
        )
        .unwrap();
    let run = |script: &str| {
        client
            .run(&crate::directorates::q::transport::RunSpec {
                argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
                env: Some(delivered.environment.clone()),
                timeout_ms: 60_000,
                ..Default::default()
            })
            .unwrap()
    };
    let output = run("ship-helper");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout.0).trim(),
        "HELPER-IN-CONTAINER"
    );
    // A second start finds the copy and checks it; a changed one refuses.
    let again = prepared.fit(
        Site::theater("box", client.clone()).unwrap(),
        &environment,
        true,
        true,
        (&keeper, Duration::from_secs(60)),
    );
    assert!(again.is_ok());
    let root = delivered.environment["TOOLS_OVERLAY_ROOT"].clone();
    run(&format!(
        "chmod -R u+w {root} && echo planted >> {root}/rules/a.md"
    ));
    let refusal = prepared
        .deliver(
            &fit,
            SessionFacts {
                environment: environment.clone(),
                handler: "codex",
                working_directory: "/work",
                key: "mission-1",
            },
            &keeper,
        )
        .err()
        .unwrap();
    assert!(
        refusal.contains("has other bytes in rules/a.md"),
        "{refusal}"
    );

    // An image with no Python: code waits, naming what is missing.
    let with_code = overlay_dir(source.path(), "coded", &[("lib/python/m.py", "x = 1\n")]);
    let list = records([record("coded", &with_code)]);
    let coded = case.prepare(&["coded"], &[], &list).unwrap().unwrap();
    let refused = coded.fit(
        Site::theater("box", client.clone()).unwrap(),
        &environment,
        true,
        true,
        (&Keeper::default(), Duration::from_secs(60)),
    );
    if run("command -v python3 || command -v python").code != Some(0) {
        let refusal = refused.err().unwrap();
        assert!(refusal.contains("python3 or python"), "{refusal}");
    }
    run("rm -rf .ai6/overlays 2>/dev/null; chmod -R u+w .ai6 2>/dev/null; rm -rf .ai6");
}

#[test]
fn a_requirements_file_holds_requirements_and_comments_only() {
    use crate::directorates::q::overlays::check_requirements;
    assert_eq!(check_requirements("# only a comment\n\n", false), Ok(()));
    assert_eq!(
        check_requirements("six==1.16 ; python_version > '3'\nrich\n", false),
        Ok(())
    );
    assert_eq!(
        check_requirements(
            "six==1.16 \\\n  --hash=sha256:aa \\\n  --hash=sha256:bb\n",
            true
        ),
        Ok(())
    );
    for (text, locked, why) in [
        ("-r other.txt\n", false, "option"),
        ("--index-url https://example.org\n", false, "option"),
        ("-e .\n", false, "option"),
        (
            "thing @ https://example.org/thing.zip\n",
            false,
            "direct URL",
        ),
        ("https://example.org/thing.zip\n", false, "direct URL"),
        ("six==1.16 --no-binary six\n", false, "--hash"),
        ("six>=1 --hash=sha256:aa\n", true, "pinned"),
    ] {
        let refusal = check_requirements(text, locked).err().unwrap();
        assert!(refusal.contains(why), "{text:?}: {refusal}");
    }
}

#[test]
fn an_overlay_md_with_no_overlay_cfg_refuses_the_session_naming_the_file() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[
            ("OVERLAY.md", "---\nname = \"tools\"\n---\nContext.\n"),
            ("bin/helper", "x\n"),
        ],
    );
    // A pin as the tree's digest would be: the refusal is the manifest's, not the pin's.
    let pin = crate::directorates::q::overlays::tree_digest(&dir, &OverlayLimits::default().tree())
        .unwrap()
        .digest;
    let list = records([OverlayRecord::new("tools", OverlayFrom::Path(dir)).pinned(pin)]);
    let refusal = case.prepare(&["tools"], &[], &list).err().unwrap();
    assert!(
        refusal.contains("does not read OVERLAY.md yet"),
        "{refusal}"
    );
    assert!(
        !case
            .state
            .path()
            .join("overlays/trees")
            .join(hex_of(&pin))
            .exists()
    );
}

#[test]
fn an_overlay_with_a_hook_refuses_the_session_naming_it() {
    let case = Case::new();
    let source = tempfile::tempdir().unwrap();
    let dir = overlay_dir(
        source.path(),
        "tools",
        &[("hook.sh", "#!/bin/sh\n"), ("kb/a.md", "a\n")],
    );
    let pin = crate::directorates::q::overlays::tree_digest(&dir, &OverlayLimits::default().tree())
        .unwrap()
        .digest;
    let list = records([OverlayRecord::new("tools", OverlayFrom::Path(dir)).pinned(pin)]);
    let refusal = case.prepare(&["tools"], &[], &list).err().unwrap();
    assert!(refusal.contains("ships hook.sh"), "{refusal}");
    assert!(
        refusal.contains("does not run overlay hooks yet"),
        "{refusal}"
    );
}
