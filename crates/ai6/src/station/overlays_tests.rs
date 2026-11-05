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
        prepared.deliver_here(SessionFacts {
            environment: environment
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            commands,
            handler,
            working_directory: &work.to_string_lossy(),
        })
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
    assert_eq!(field.overlays.roots, [root.clone()]);

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
    let refusal = prepared
        .deliver_here(SessionFacts {
            environment: BTreeMap::new(),
            commands: true,
            handler: "codex",
            working_directory: &case.state.path().to_string_lossy(),
        })
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
    let mut limits = OverlayLimits::default();
    limits.unpinned = true;
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
