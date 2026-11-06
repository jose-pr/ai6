use std::collections::BTreeMap;
use std::path::Path;

use super::*;
use crate::directorates::q::LoadoutCatalog;

fn manifest(text: &str) -> Document {
    read_document(text.as_bytes(), &ReadLimits::default()).unwrap()
}

fn record(name: &str, pinned: bool) -> (String, OverlayRecord) {
    let mut record = OverlayRecord::new(
        name,
        OverlayFrom::Path(format!("/srv/overlays/{name}").into()),
    );
    if pinned {
        record = record.pinned(Digest::of(name.as_bytes()));
    }
    (record.identity(), record)
}

/// `engineering` (500), `rust` requiring it (600), `python` requiring it (400) and `docs`
/// (500): every one pinned, each manifest by identity.
fn catalog() -> (BTreeMap<String, OverlayRecord>, BTreeMap<String, Document>) {
    let manifests: BTreeMap<String, Document> = [
        ("engineering", "name = \"engineering\"\n"),
        ("rust", "requires = [\"engineering\"]\npriority = 600\n"),
        ("python", "requires = [\"Engineering\"]\npriority = 400\n"),
        ("docs", "name = \"docs\"\n"),
    ]
    .into_iter()
    .map(|(identity, text)| (identity.to_owned(), manifest(text)))
    .collect();
    let records = manifests.keys().map(|name| record(name, true)).collect();
    (records, manifests)
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn issued(loadout: &[&str], operation: &[&str]) -> Result<IssuedOverlays, OverlayRefusal> {
    let (records, manifests) = catalog();
    issue_overlays(
        &names(loadout),
        &names(operation),
        &records,
        &manifests,
        false,
    )
}

fn order(issued: &IssuedOverlays) -> Vec<&str> {
    issued
        .overlays
        .iter()
        .map(|overlay| overlay.identity.as_str())
        .collect()
}

#[test]
fn overlays_issue_in_composition_order_with_requires_first_to_install() {
    let issued = issued(&["rust"], &["python", "docs"]).unwrap();
    assert_eq!(
        issued.install_order,
        ["engineering", "rust", "python", "docs"]
    );
    // (priority, name, identity): python 400, docs and engineering 500 by name, rust 600.
    assert_eq!(order(&issued), ["python", "docs", "engineering", "rust"]);
    assert!(issued.weakenings.is_empty());
}

#[test]
fn an_overlay_named_twice_or_required_twice_is_issued_once() {
    let issued = issued(&["rust", "python"], &["Engineering", "rust", "team_review"]);
    assert!(matches!(
        issued,
        Err(OverlayRefusal::Unknown {
            list: OverlayList::Operation,
            ..
        })
    ));
    let issued = super::tests::issued(&["rust", "python"], &["Engineering", "RUST"]).unwrap();
    assert_eq!(order(&issued), ["python", "engineering", "rust"]);
    assert_eq!(issued.install_order, ["engineering", "rust", "python"]);
}

#[test]
fn an_operation_adds_to_a_loadout_s_overlays_and_cannot_take_one_away() {
    let alone = issued(&["rust"], &[]).unwrap();
    assert_eq!(order(&alone), ["engineering", "rust"]);
    let with = issued(&["rust"], &["docs"]).unwrap();
    for identity in order(&alone) {
        assert!(order(&with).contains(&identity), "{identity} stays");
    }
    // A list has no way to say "not this one": such a name is no overlay name.
    for removal in ["-rust", "!rust", "~rust", "rust-"] {
        assert_eq!(
            issued(&["rust"], &[removal]),
            Err(OverlayRefusal::InvalidName {
                list: OverlayList::Operation,
                name: removal.to_owned()
            }),
            "{removal}"
        );
    }
}

#[test]
fn a_cycle_a_missing_requirement_and_an_unknown_name_refuse_the_session() {
    let (mut records, mut manifests) = catalog();
    manifests.insert("a".into(), manifest("requires = [\"b\"]\n"));
    manifests.insert("b".into(), manifest("requires = [\"a\"]\n"));
    manifests.insert("needy".into(), manifest("requires = [\"absent\"]\n"));
    for name in ["a", "b", "needy"] {
        records.extend([record(name, true)]);
    }
    let refused =
        |loadout: &[&str]| issue_overlays(&names(loadout), &[], &records, &manifests, false);
    assert_eq!(
        refused(&["a"]),
        Err(OverlayRefusal::Cycle(names(&["a", "b", "a"])))
    );
    assert_eq!(
        refused(&["needy"]),
        Err(OverlayRefusal::Missing {
            by: "needy".into(),
            name: "absent".into()
        })
    );
    assert!(matches!(
        refused(&["nobody"]),
        Err(OverlayRefusal::Unknown {
            list: OverlayList::Loadout,
            ..
        })
    ));
    // A record whose manifest was not read is missing too: nothing is issued half-read.
    manifests.remove("engineering");
    assert_eq!(
        issue_overlays(&names(&["rust"]), &[], &records, &manifests, false),
        Err(OverlayRefusal::Missing {
            by: "rust".into(),
            name: "engineering".into()
        })
    );
}

#[test]
fn an_unpinned_overlay_is_refused_unless_allowed_and_then_reported() {
    let (mut records, manifests) = catalog();
    records.extend([record("docs", false)]);
    let request = |allow| issue_overlays(&names(&["docs"]), &[], &records, &manifests, allow);
    assert_eq!(request(false), Err(OverlayRefusal::Unpinned("docs".into())));
    assert!(
        request(false)
            .unwrap_err()
            .to_string()
            .contains("station.overlays.unpinned")
    );
    let issued = request(true).unwrap();
    assert_eq!(
        issued.weakenings,
        [Weakening {
            id: "unpinned".into(),
            subject: "docs".into()
        }]
    );
}

/// Writes the files of a small overlay, anew for these tests, under `root`.
fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

const ENGINEERING: &[(&str, &str)] = &[
    (
        "overlay.cfg",
        "name = \"engineering\"\ndescription = \"How work is planned and committed.\"\n\
         routing = [\"- Writing a plan -> `$ENGINEERING_OVERLAY_ROOT/flows/PLAN.md`\"]\n",
    ),
    ("flows/PLAN.md", "# Plans\n\nOne file per plan.\n"),
    (
        "rules/commits.md",
        "- **Commits** are logical, one concern each.\n",
    ),
    (
        "skills/review-diff/SKILL.md",
        "---\nname: review-diff\ndescription: Read a diff for correctness.\n---\nRead it twice.\n",
    ),
];

const RUST: &[(&str, &str)] = &[
    (
        "overlay.cfg",
        "requires = [\"engineering\"]\npriority = 600\n",
    ),
    ("kb/RUST.md", "# Rust\n\nClippy with warnings denied.\n"),
    ("bin/check-all", "#!/bin/sh\ncargo clippy --all-targets\n"),
];

#[test]
fn reading_an_overlay_gives_its_tree_digest_manifest_and_checked_skills() {
    let root = tempfile::tempdir().unwrap();
    let (engineering, rust) = (root.path().join("engineering"), root.path().join("rust"));
    write(&engineering, ENGINEERING);
    write(&rust, RUST);
    write(
        &engineering,
        &[
            (".git/HEAD", "ref: refs/heads/main\n"),
            ("lib/__pycache__/x.pyc", "x"),
        ],
    );
    let limits = OverlayLimits::default();
    let read = read_overlay(&engineering, &limits).unwrap();
    assert!(read.usable(), "{read:?}");
    assert_eq!(
        read.digest.files.keys().collect::<Vec<_>>(),
        [
            "flows/PLAN.md",
            "overlay.cfg",
            "rules/commits.md",
            "skills/review-diff/SKILL.md"
        ],
        "excluded names are no members"
    );
    assert_eq!(read.skills, [("review-diff".to_owned(), Ok(()))]);
    let tree = read.tree();
    assert_eq!(rules_of(&read.manifest, &tree), ["rules/commits.md"]);
    // The digest pin-skill prints is the same function's.
    assert_eq!(
        crate::ProjectManifest::read_tree(&engineering)
            .unwrap()
            .digest()
            .to_string(),
        read.digest.digest.to_string()
    );
    let rust_read = read_overlay(&rust, &limits).unwrap();
    assert_eq!(
        default_paths(&rust_read.manifest, &rust_read.tree())[0]["variable"],
        "PATH"
    );
    // The two compose: engineering first, rust after it.
    let manifests = BTreeMap::from([
        ("engineering".to_owned(), read.manifest.clone()),
        ("rust".to_owned(), rust_read.manifest.clone()),
    ]);
    let records = BTreeMap::from([record("engineering", true), record("rust", true)]);
    let issued = issue_overlays(&names(&["rust"]), &[], &records, &manifests, false).unwrap();
    assert_eq!(order(&issued), ["engineering", "rust"]);
}

#[test]
fn a_changed_file_changes_the_tree_digest() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), RUST);
    let limits = OverlayLimits::default().tree();
    let before = tree_digest(root.path(), &limits).unwrap().digest;
    write(
        root.path(),
        &[("kb/RUST.md", "# Rust\n\nClippy with warnings denied!\n")],
    );
    let edited = tree_digest(root.path(), &limits).unwrap().digest;
    assert_ne!(before, edited, "an edited file");
    write(root.path(), &[("kb/NEW.md", "new\n")]);
    let added = tree_digest(root.path(), &limits).unwrap().digest;
    assert_ne!(edited, added, "an added file");
    std::fs::rename(
        root.path().join("kb/NEW.md"),
        root.path().join("kb/OTHER.md"),
    )
    .unwrap();
    assert_ne!(
        added,
        tree_digest(root.path(), &limits).unwrap().digest,
        "a renamed file"
    );
    // An empty directory and an excluded one change nothing.
    let now = tree_digest(root.path(), &limits).unwrap().digest;
    std::fs::create_dir_all(root.path().join("empty/inner")).unwrap();
    write(root.path(), &[("node_modules/x/index.js", "x\n")]);
    assert_eq!(now, tree_digest(root.path(), &limits).unwrap().digest);
}

#[test]
fn a_tree_past_a_limit_is_refused_naming_the_limit() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), RUST);
    let mut limits = OverlayLimits::default().tree();
    limits.files = crate::bounds::Limit::at(2);
    let refused = tree_digest(root.path(), &limits).unwrap_err();
    assert_eq!(refused.kind, DigestErrorKind::TooLarge);
    assert!(refused.path.contains("more than 2 files"), "{refused}");
}

#[test]
fn a_skill_that_grants_tools_makes_an_overlay_unusable() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &[(
            "skills/shell/SKILL.md",
            "---\nname: shell\ndescription: Runs things.\nallowed-tools: Bash\n---\nGo.\n",
        )],
    );
    let read = read_overlay(root.path(), &OverlayLimits::default()).unwrap();
    assert!(!read.usable());
    assert!(
        read.skills[0]
            .1
            .as_ref()
            .unwrap_err()
            .contains("allowed-tools")
    );
}

#[test]
fn a_loadout_issues_overlays_its_document_defines() {
    let pin = format!("sha256:{}", "a".repeat(64));
    let text = format!(
        "[overlay.engineering]\nsource = \"https://example.org/agents.git@v1#overlays/engineering\"\n\
         pin = \"{pin}\"\nsecrets = [\"agents-deploy-key\"]\n\n[overlay.rust]\npath = \"/srv/overlays/rust\"\n\n\
         [loadout.007]\noverlays = [\"rust\", \"engineering\"]\n"
    );
    let catalog: LoadoutCatalog = text.parse().unwrap();
    assert_eq!(
        catalog.for_agent("007").unwrap().overlay_names(),
        ["rust", "engineering"]
    );
    let engineering = catalog.overlay("Engineering").unwrap();
    assert_eq!(engineering.pin().unwrap().to_string(), pin);
    assert_eq!(engineering.secrets(), ["agents-deploy-key"]);
    assert!(
        matches!(engineering.from(), OverlayFrom::Source(source) if source.kind == SourceKind::Git)
    );
    assert_eq!(catalog.overlay("rust").unwrap().pin(), None);
    for (bad, says) in [
        (
            "[loadout.007]\noverlays = [\"absent\"]\n[overlay.x]\npath = \"/x\"\n",
            "not defined",
        ),
        (
            "[overlay.x]\npath = \"/x\"\n[loadout.007]\noverlays = [\"x\", \"X\"]\n",
            "named twice",
        ),
        (
            "[overlay.x]\npath = \"/x\"\nsource = \"/y\"\n",
            "exactly one",
        ),
        ("[overlay.x]\n", "exactly one"),
        ("[overlay.project]\npath = \"/x\"\n", "a name is"),
        ("[overlay.x]\npath = \"/x\"\npin = \"sha1:00\"\n", "pin"),
        (
            "[overlay.Team_Review]\npath = \"/x\"\n[overlay.team-review]\npath = \"/y\"\n",
            "one overlay",
        ),
        (
            "[overlay.x]\nsource = \"https://h.example/a.git#../..\"\n",
            "source",
        ),
    ] {
        let refused = bad.parse::<LoadoutCatalog>().unwrap_err();
        assert!(
            refused.reason().is_some_and(|reason| reason.contains(says)),
            "{bad}: {refused:?}"
        );
    }
}

#[test]
fn an_operation_names_overlays_by_identity_once_each() {
    let roster = |text: &str| {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            &[("operations.toml", text), ("agents.toml", "[agent.007]\n")],
        );
        crate::directorates::m::roster::Roster::load(directory.path())
    };
    let ok = roster("[operation.G]\noverlays = [\"rust\", \"docs\"]\n").unwrap();
    assert_eq!(ok.operation("G").unwrap().overlays, ["rust", "docs"]);
    for bad in ["[\"-rust\"]", "[\"rust\", \"Rust\"]", "[\"project\"]"] {
        assert!(
            roster(&format!("[operation.G]\noverlays = {bad}\n")).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn an_overlay_md_with_no_overlay_cfg_is_refused_and_never_read_as_no_manifest() {
    let root = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let file = root.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    };
    write("OVERLAY.md", "---\nrequires = [\"base\"]\n---\nContext.\n");
    write("bin/helper", "#!/bin/sh\n");
    let refusal = read_overlay(root.path(), &OverlayLimits::default()).unwrap_err();
    assert!(refusal.contains("OVERLAY.md"), "{refusal}");
    assert!(
        refusal.contains("does not read OVERLAY.md yet"),
        "{refusal}"
    );
    // With an overlay.cfg beside it, the manifest is that file, as this edition reads it.
    write("overlay.cfg", "name = \"tools\"\n");
    let read = read_overlay(root.path(), &OverlayLimits::default()).unwrap();
    assert_eq!(read.manifest["name"], "tools");
}

#[test]
fn an_overlay_asking_for_hooks_is_refused_naming_what_asks() {
    for (path, text, named) in [
        (
            "overlay.cfg",
            "[hooks]\npre-tool = \"hooks/check.sh\"\n",
            "declares [hooks]",
        ),
        ("hook.sh", "#!/bin/sh\nexit 0\n", "ships hook.sh"),
        ("hook.py", "print()\n", "ships hook.py"),
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(path), text).unwrap();
        std::fs::write(root.path().join("rules.md"), "a\n").unwrap();
        let refusal = read_overlay(root.path(), &OverlayLimits::default()).unwrap_err();
        assert!(refusal.contains(named), "{path}: {refusal}");
        assert!(
            refusal.contains("does not run overlay hooks yet"),
            "{refusal}"
        );
    }
    // Only a root script by a default script's extension is one: these are files like any other.
    let root = tempfile::tempdir().unwrap();
    for path in ["hook.txt", "hooks.sh"] {
        std::fs::write(root.path().join(path), "x\n").unwrap();
    }
    std::fs::create_dir_all(root.path().join("kb")).unwrap();
    std::fs::write(root.path().join("kb/hook.sh"), "x\n").unwrap();
    assert!(read_overlay(root.path(), &OverlayLimits::default()).is_ok());
}
