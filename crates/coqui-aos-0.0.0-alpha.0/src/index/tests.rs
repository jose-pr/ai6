#![allow(clippy::field_reassign_with_default)]

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::fs;

use crate::{KnowledgeOverlay, KnowledgeSettings, KnowledgeTree};

use super::*;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A folder that is removed with it.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(alloc::format!(
            "canoa-kb-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn index(&self) -> PathBuf {
        self.0.join("index")
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.index())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Vectors of word counts hashed into eight slots: texts sharing words are near each other.
struct Words {
    model: &'static str,
    calls: Cell<usize>,
    inputs: RefCell<Vec<String>>,
    fail_on_call: Cell<Option<usize>>,
}

impl Words {
    fn new(model: &'static str) -> Self {
        Self {
            model,
            calls: Cell::new(0),
            inputs: RefCell::new(Vec::new()),
            fail_on_call: Cell::new(None),
        }
    }
}

impl Embedder for Words {
    fn model(&self) -> &str {
        self.model
    }

    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        self.calls.set(self.calls.get() + 1);
        if self.fail_on_call.get() == Some(self.calls.get()) {
            return Err(Error::Embed("the provider went away".into()));
        }
        self.inputs.borrow_mut().extend(inputs.iter().cloned());
        Ok(inputs
            .iter()
            .map(|input| {
                let mut vector = vec![0.0f32; 8];
                for word in lexical::words(input) {
                    let slot = word
                        .bytes()
                        .fold(7usize, |hash, byte| hash * 31 + usize::from(byte))
                        % 8;
                    vector[slot] += 1.0;
                }
                if vector.iter().all(|value| *value == 0.0) {
                    vector[0] = 1.0;
                }
                vector
            })
            .collect())
    }
}

fn corpus(files: &[(&str, &str)]) -> Corpus {
    let mut corpus = Corpus::new();
    for (path, text) in files {
        corpus.insert(path, text.as_bytes().to_vec()).unwrap();
    }
    corpus
}

fn small() -> Corpus {
    corpus(&[
        (
            "kb/rust.md",
            "# Rust\n\nCargo builds crates. Clippy lints them.\n\n## Testing\n\nRun cargo test for the unit tests.\n",
        ),
        (
            "kb/python.md",
            "# Python\n\nThe interpreter runs scripts. Pip installs packages.\n",
        ),
        (
            "rules/style.md",
            "Prefer short functions.\nName things for what they hold.\n",
        ),
    ])
}

fn paths(hits: &[Hit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.path.as_str()).collect()
}

#[test]
fn passages_keep_their_path_headings_and_line_ranges() {
    let text = "intro\n# One\nfirst\nsecond\n## Two\nthird\n";
    let pieces = chunk::chunk(text, true, 1024, 0);
    let seen: Vec<(&str, usize, usize)> = pieces
        .iter()
        .map(|piece| (piece.heading.as_str(), piece.from, piece.to))
        .collect();
    assert_eq!(seen, [("", 1, 1), ("One", 2, 4), ("One > Two", 5, 6)]);
    assert_eq!(pieces[1].text, "# One\nfirst\nsecond");

    let mut corpus = Corpus::new();
    corpus.insert("kb/a.md", text.as_bytes().to_vec()).unwrap();
    let scratch = Scratch::new();
    Index::create(
        &scratch.index(),
        Settings::default(),
        &corpus,
        None,
        &Options::default(),
    )
    .unwrap();
    let hits = Index::open(&scratch.index())
        .unwrap()
        .search("third", &SearchOptions::default(), None)
        .unwrap();
    assert_eq!(
        (
            hits[0].path.as_str(),
            hits[0].heading.as_str(),
            hits[0].from_line,
            hits[0].to_line
        ),
        ("kb/a.md", "One > Two", 5, 6)
    );
}

#[test]
fn a_hash_in_a_code_fence_is_not_a_heading_and_a_bare_heading_is_no_passage() {
    let text = "# Top\n## Empty\n## Real\n```sh\n# a comment\n```\nbody\n";
    let pieces = chunk::chunk(text, true, 1024, 0);
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].heading, "Top > Real");
    assert_eq!((pieces[0].from, pieces[0].to), (3, 7));
    // A file that is not Markdown has no headings.
    let plain = chunk::chunk("# not a heading\ntext\n", false, 1024, 0);
    assert_eq!(plain[0].heading, "");
    // A trailing `#` that is part of the title stays.
    assert_eq!(chunk::chunk("# C#\nbody\n", true, 1024, 0)[0].heading, "C#");
    assert_eq!(
        chunk::chunk("## Title ##\nbody\n", true, 1024, 0)[0].heading,
        "Title"
    );
}

#[test]
fn size_cuts_a_section_at_lines_and_overlap_repeats_the_tail() {
    let text = "aaaa\nbbbb\ncccc\ndddd\neeee\n";
    let whole: Vec<(usize, usize)> = chunk::chunk(text, false, 9, 0)
        .iter()
        .map(|piece| (piece.from, piece.to))
        .collect();
    assert_eq!(whole, [(1, 2), (3, 4), (5, 5)]);
    let overlapping: Vec<(usize, usize)> = chunk::chunk(text, false, 14, 4)
        .iter()
        .map(|piece| (piece.from, piece.to))
        .collect();
    assert_eq!(overlapping, [(1, 3), (3, 5)]);
    for piece in chunk::chunk(text, false, 14, 4) {
        assert!(piece.text.len() <= 14);
    }
}

#[test]
fn a_line_longer_than_the_budget_is_cut_inside_it_on_a_character() {
    let text = "ééééééééé ééééééééé\n";
    let pieces = chunk::chunk(text, false, 8, 0);
    assert!(pieces.len() > 2);
    assert!(
        pieces
            .iter()
            .all(|piece| piece.text.len() <= 8 && piece.from == 1 && piece.to == 1)
    );
    let joined: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
    assert_eq!(joined.trim_end(), text.trim_end());
}

#[test]
fn a_carriage_return_is_dropped_only_before_a_line_feed() {
    let pieces = chunk::chunk("# T\r\nbody\r\nmore\r\n", true, 1024, 0);
    assert_eq!(pieces[0].text, "# T\nbody\nmore");
    let pieces = chunk::chunk("# T\nbody\rmore\nlast\r", true, 1024, 0);
    assert_eq!(pieces[0].text, "# T\nbody\rmore\nlast\r");
    assert_eq!((pieces[0].from, pieces[0].to), (1, 3));
}

#[test]
fn settings_are_checked() {
    let scratch = Scratch::new();
    let create = |settings: Settings| {
        Index::create(
            &scratch.index(),
            settings,
            &small(),
            None,
            &Options::default(),
        )
    };
    let mut settings = Settings::default();
    settings.chunk_bytes = 2;
    assert!(matches!(create(settings), Err(Error::Settings(_))));
    let mut settings = Settings::default();
    settings.overlap_bytes = 1024;
    assert!(matches!(create(settings), Err(Error::Settings(_))));
    let mut settings = Settings::default();
    settings.include = vec!["[".into()];
    assert!(matches!(create(settings), Err(Error::Settings(_))));
    let mut settings = Settings::default();
    settings.document_prefix = "x".repeat(1022);
    assert!(matches!(create(settings), Err(Error::Settings(_))));
    assert!(!scratch.index().exists());
}

#[test]
fn no_include_filter_selects_every_file_and_the_filters_narrow_it() {
    let selector = Selector::new(&[], &[]).unwrap();
    for yes in ["kb/a.md", "flows/x.md", "CONTEXT.md", "docs/n.md"] {
        assert!(selector.selects(yes), "{yes}");
    }
    let custom =
        Selector::new(&["*.md".into()], &["drafts/**".into(), "secret.md".into()]).unwrap();
    assert!(custom.selects("a/b/c.md") && custom.selects("c.md"));
    assert!(
        !custom.selects("drafts/x.md")
            && !custom.selects("a/secret.md")
            && !custom.selects("c.txt")
    );
}

#[test]
fn a_lexical_index_ranks_by_words_with_no_model() {
    let scratch = Scratch::new();
    let report = Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        report.added,
        ["kb/python.md", "kb/rust.md", "rules/style.md"]
    );
    assert_eq!(report.generation, Some(1));
    let index = Index::open(&scratch.index()).unwrap();
    assert_eq!(index.settings().model, None);

    let hits = index
        .search("cargo test", &SearchOptions::default(), None)
        .unwrap();
    assert_eq!(hits[0].path, "kb/rust.md");
    assert_eq!(hits[0].heading, "Rust > Testing");
    assert!(hits[0].score > 0.0);
    // The rarer word counts for more: `pip` is in one passage only.
    assert_eq!(
        paths(
            &index
                .search("pip", &SearchOptions::default(), None)
                .unwrap()
        ),
        ["kb/python.md"]
    );
    // Matches are ordered and limited, and a query with no known word finds nothing.
    let mut options = SearchOptions::default();
    options.top = 1;
    assert_eq!(index.search("the", &options, None).unwrap().len(), 1);
    assert!(
        index
            .search("zebra", &SearchOptions::default(), None)
            .unwrap()
            .is_empty()
    );
    options.top = 0;
    assert!(matches!(
        index.search("a", &options, None),
        Err(Error::Settings(_))
    ));
    // Not in a default folder: not indexed.
}

#[test]
fn a_search_can_be_limited_to_a_file_or_a_folder() {
    let scratch = Scratch::new();
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    let index = Index::open(&scratch.index()).unwrap();
    let mut options = SearchOptions::default();
    options.path = Some("rules".into());
    assert_eq!(
        paths(&index.search("functions", &options, None).unwrap()),
        ["rules/style.md"]
    );
    assert!(index.search("cargo", &options, None).unwrap().is_empty());
    options.path = Some("kb/rust.md".into());
    assert_eq!(
        paths(&index.search("cargo", &options, None).unwrap()),
        ["kb/rust.md", "kb/rust.md"]
    );
    // A prefix of a name is not a folder.
    options.path = Some("kb/rus".into());
    assert!(index.search("cargo", &options, None).unwrap().is_empty());
    options.path = Some("../kb".into());
    assert!(matches!(
        index.search("cargo", &options, None),
        Err(Error::Path(_))
    ));
}

#[test]
fn an_update_embeds_only_what_changed() {
    let scratch = Scratch::new();
    let embedder = Words::new("m-1");
    let report = Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(report.passages_new, 4);
    assert_eq!(embedder.inputs.borrow().len(), 4);

    embedder.inputs.borrow_mut().clear();
    let mut next = small();
    next.insert(
        "kb/python.md",
        b"# Python\n\nVirtual environments isolate packages.\n".to_vec(),
    )
    .unwrap();
    next.insert("kb/new.md", b"A brand new note about lasagna.\n".to_vec())
        .unwrap();
    let report = Index::update(
        &scratch.index(),
        &next,
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(report.changed, ["kb/python.md"]);
    assert_eq!(report.added, ["kb/new.md"]);
    assert_eq!(report.unchanged, ["kb/rust.md", "rules/style.md"]);
    assert_eq!(report.generation, Some(2));
    assert_eq!(report.passages_kept, 3);
    assert_eq!(report.passages_new, 2);
    assert_eq!(embedder.inputs.borrow().len(), 2);

    let index = Index::open(&scratch.index()).unwrap();
    assert!(
        index
            .search("interpreter", &SearchOptions::default(), None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        paths(
            &index
                .search("lasagna", &SearchOptions::default(), None)
                .unwrap()
        ),
        ["kb/new.md"]
    );
    // Only the committed generation is left.
    assert_eq!(scratch.names(), ["CURRENT", "gen-000002"]);

    // Nothing changed: nothing embedded, and the passages are the same.
    embedder.inputs.borrow_mut().clear();
    let report = Index::update(
        &scratch.index(),
        &next,
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        (report.passages_new, embedder.inputs.borrow().len()),
        (0, 0)
    );
    assert!(report.added.is_empty() && report.changed.is_empty());
}

#[test]
fn an_update_keeps_a_missing_file_until_it_prunes() {
    let scratch = Scratch::new();
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    let without = corpus(&[
        ("kb/rust.md", small().text("kb/rust.md").unwrap()),
        ("kb/python.md", small().text("kb/python.md").unwrap()),
    ]);

    let kept = Index::update(&scratch.index(), &without, None, &Options::default()).unwrap();
    assert_eq!(kept.retained, ["rules/style.md"]);
    assert!(kept.removed.is_empty());
    let index = Index::open(&scratch.index()).unwrap();
    assert!(index.paths().any(|path| path == "rules/style.md"));
    assert_eq!(
        paths(
            &index
                .search("functions", &SearchOptions::default(), None)
                .unwrap()
        ),
        ["rules/style.md"]
    );

    let mut options = Options::default();
    options.prune = true;
    let pruned = Index::update(&scratch.index(), &without, None, &options).unwrap();
    assert_eq!(pruned.removed, ["rules/style.md"]);
    let index = Index::open(&scratch.index()).unwrap();
    assert!(index.paths().all(|path| path != "rules/style.md"));
    assert!(
        index
            .search("functions", &SearchOptions::default(), None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_dry_run_reports_and_writes_nothing() {
    let scratch = Scratch::new();
    let embedder = Words::new("m-1");
    let mut options = Options::default();
    options.dry_run = true;
    let report = Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        Some(&embedder),
        &options,
    )
    .unwrap();
    assert!(report.dry_run);
    assert_eq!(report.added.len(), 3);
    assert_eq!(report.passages_new, 4);
    assert_eq!(embedder.calls.get(), 0);
    assert!(!scratch.index().exists());

    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    let before = scratch.names();
    let calls = embedder.calls.get();
    let mut next = small();
    next.insert("kb/rust.md", b"# Rust\n\nchanged\n".to_vec())
        .unwrap();
    let report = Index::update(&scratch.index(), &next, Some(&embedder), &options).unwrap();
    assert_eq!(report.changed, ["kb/rust.md"]);
    assert_eq!(report.passages_new, 1);
    assert_eq!(report.generation, Some(1));
    assert_eq!((scratch.names(), embedder.calls.get()), (before, calls));
    // No embedder is needed to look.
    assert!(Index::update(&scratch.index(), &next, None, &options).is_ok());
    let rebuilt =
        Index::rebuild(&scratch.index(), Settings::default(), &next, None, &options).unwrap();
    assert!(rebuilt.dry_run);
    assert_eq!(
        Index::open(&scratch.index())
            .unwrap()
            .settings()
            .model
            .as_deref(),
        Some("m-1")
    );
}

#[test]
fn a_failed_create_leaves_nothing_and_a_failed_update_or_rebuild_leaves_the_committed_index() {
    let scratch = Scratch::new();
    let embedder = Words::new("m-1");
    let mut options = Options::default();
    options.batch_size = 1;

    embedder.fail_on_call.set(Some(3));
    assert!(matches!(
        Index::create(
            &scratch.index(),
            Settings::default(),
            &small(),
            Some(&embedder),
            &options
        ),
        Err(Error::Embed(_))
    ));
    assert!(!Index::exists(&scratch.index()));
    assert!(
        scratch.names().is_empty() || scratch.names().iter().all(|name| !name.starts_with("gen-"))
    );

    embedder.fail_on_call.set(None);
    embedder.calls.set(0);
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        Some(&embedder),
        &options,
    )
    .unwrap();
    let before = scratch.names();
    let index = Index::open(&scratch.index()).unwrap();
    let wanted = index
        .search("cargo", &SearchOptions::default(), Some(&embedder))
        .unwrap();

    let mut next = small();
    next.insert("kb/rust.md", b"# Rust\n\nreplaced text\n".to_vec())
        .unwrap();
    next.insert("kb/python.md", b"# Python\n\nreplaced too\n".to_vec())
        .unwrap();
    embedder.calls.set(0);
    embedder.fail_on_call.set(Some(2));
    assert!(Index::update(&scratch.index(), &next, Some(&embedder), &options).is_err());
    embedder.calls.set(0);
    assert!(
        Index::rebuild(
            &scratch.index(),
            Settings::default(),
            &next,
            Some(&embedder),
            &options
        )
        .is_err()
    );
    embedder.fail_on_call.set(None);

    assert_eq!(scratch.names(), before);
    let index = Index::open(&scratch.index()).unwrap();
    assert_eq!(index.generation(), 1);
    assert_eq!(
        index
            .search("cargo", &SearchOptions::default(), Some(&embedder))
            .unwrap(),
        wanted
    );
}

#[test]
fn a_half_written_generation_is_never_read_and_is_cleared_by_the_next_write() {
    let scratch = Scratch::new();
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    let dir = scratch.index();
    fs::create_dir(dir.join("gen-000002.tmp")).unwrap();
    fs::write(
        dir.join("gen-000002.tmp").join("chunks.jsonl"),
        "{ not json",
    )
    .unwrap();
    fs::create_dir(dir.join("gen-000007")).unwrap();
    fs::write(dir.join("CURRENT.tmp"), "gen-000007").unwrap();
    assert_eq!(Index::open(&dir).unwrap().generation(), 1);

    let report = Index::update(&dir, &small(), None, &Options::default()).unwrap();
    // The next number is past every folder met, and only the committed one remains.
    assert_eq!(report.generation, Some(8));
    assert_eq!(scratch.names(), ["CURRENT", "gen-000008"]);
}

#[test]
fn the_model_and_prefixes_are_saved_and_used() {
    let scratch = Scratch::new();
    let embedder = Words::new("model-a");
    let mut settings = Settings::default();
    settings.document_prefix = "passage: ".into();
    settings.query_prefix = "query: ".into();
    Index::create(
        &scratch.index(),
        settings,
        &small(),
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    assert!(
        embedder
            .inputs
            .borrow()
            .iter()
            .all(|input| input.starts_with("passage: "))
    );

    let index = Index::open(&scratch.index()).unwrap();
    assert_eq!(index.settings().model.as_deref(), Some("model-a"));
    assert_eq!(index.settings().dimensions, Some(8));
    assert_eq!(index.settings().document_prefix, "passage: ");

    embedder.inputs.borrow_mut().clear();
    let hits = index
        .search(
            "cargo builds crates",
            &SearchOptions::default(),
            Some(&embedder),
        )
        .unwrap();
    assert_eq!(
        embedder.inputs.borrow().as_slice(),
        ["query: cargo builds crates"]
    );
    assert_eq!(hits[0].path, "kb/rust.md");
    assert!(hits[0].score > 0.0 && hits[0].score <= 1.0001);
    // The prefix is inside the passage budget.
    assert!(
        embedder
            .inputs
            .borrow()
            .iter()
            .all(|input| input.len() <= 1024)
    );

    // Another model cannot be mixed in by an update or a search.
    let other = Words::new("model-b");
    let mut next = small();
    next.insert("kb/rust.md", b"# Rust\n\nnew\n".to_vec())
        .unwrap();
    assert!(matches!(
        Index::update(&scratch.index(), &next, Some(&other), &Options::default()),
        Err(Error::Model(_))
    ));
    assert!(matches!(
        Index::update(&scratch.index(), &next, None, &Options::default()),
        Err(Error::Model(_))
    ));
    assert!(matches!(
        index.search("x", &SearchOptions::default(), Some(&other)),
        Err(Error::Model(_))
    ));
    // A rebuild is how the model changes.
    Index::rebuild(
        &scratch.index(),
        index.settings().clone(),
        &next,
        Some(&other),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        Index::open(&scratch.index())
            .unwrap()
            .settings()
            .model
            .as_deref(),
        Some("model-b")
    );
}

#[test]
fn the_modes_of_a_search() {
    let scratch = Scratch::new();
    let embedder = Words::new("m");
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    let words = Index::open(&scratch.index()).unwrap();
    let mut options = SearchOptions::default();
    options.mode = Mode::Semantic;
    assert!(matches!(
        words.search("cargo", &options, Some(&embedder)),
        Err(Error::Mode(_))
    ));
    // Auto falls to words for an index with none.
    assert!(
        !words
            .search("cargo", &SearchOptions::default(), Some(&embedder))
            .unwrap()
            .is_empty()
    );

    Index::rebuild(
        &scratch.index(),
        Settings::default(),
        &small(),
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    let both = Index::open(&scratch.index()).unwrap();
    assert!(matches!(
        both.search("cargo", &options, None),
        Err(Error::Mode(_))
    ));
    options.mode = Mode::Lexical;
    embedder.inputs.borrow_mut().clear();
    assert!(
        !both
            .search("cargo", &options, Some(&embedder))
            .unwrap()
            .is_empty()
    );
    assert!(embedder.inputs.borrow().is_empty());
    // Auto without an embedder is by words, and a floor applies to the scorer in use.
    assert!(
        !both
            .search("cargo", &SearchOptions::default(), None)
            .unwrap()
            .is_empty()
    );
    let mut high = SearchOptions::default();
    high.min_score = Some(1.5);
    assert!(
        both.search("cargo", &high, Some(&embedder))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_rebuild_replaces_the_settings_and_every_passage() {
    let scratch = Scratch::new();
    let embedder = Words::new("m");
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    embedder.inputs.borrow_mut().clear();
    let mut settings = Index::open(&scratch.index()).unwrap().settings().clone();
    settings.chunk_bytes = 24;
    settings.exclude = vec!["rules/**".into()];
    let report = Index::rebuild(
        &scratch.index(),
        settings,
        &small(),
        Some(&embedder),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(report.removed, ["rules/style.md"]);
    assert!(report.unchanged.is_empty());
    assert_eq!(embedder.inputs.borrow().len(), report.passages_new);
    let index = Index::open(&scratch.index()).unwrap();
    assert_eq!(index.settings().chunk_bytes, 24);
    assert!(index.len() > 2);
    // No embedder makes an index of words.
    Index::rebuild(
        &scratch.index(),
        index.settings().clone(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        Index::open(&scratch.index()).unwrap().settings().model,
        None
    );
}

#[test]
fn create_refuses_an_existing_index_and_update_a_missing_one() {
    let scratch = Scratch::new();
    assert!(matches!(
        Index::update(&scratch.index(), &small(), None, &Options::default()),
        Err(Error::Missing(_))
    ));
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    assert!(matches!(
        Index::create(
            &scratch.index(),
            Settings::default(),
            &small(),
            None,
            &Options::default()
        ),
        Err(Error::Exists(_))
    ));
    let report = Index::refresh(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
        None,
    )
    .unwrap();
    assert_eq!(report.generation, Some(2));
    assert_eq!(report.unchanged.len(), 3);
}

#[test]
fn an_index_is_where_the_overlay_version_puts_it() {
    let scratch = Scratch::new();
    let state = &scratch.0;
    let old = index_dir(state, "team/kb", "sha256:aaaa");
    let new = index_dir(state, "team/kb", "sha256:bbbb");
    assert_ne!(old, new);
    assert_eq!(old.parent(), new.parent());
    assert!(old.starts_with(state.join("kb")));
    assert!(
        !old.to_string_lossy().contains(':')
            || cfg!(windows) && old.to_string_lossy().matches(':').count() == 1
    );
    let mut options = Options::default();
    options.source = Some("sha256:aaaa".into());
    Index::create(&old, Settings::default(), &small(), None, &options).unwrap();
    options.source = Some("sha256:bbbb".into());
    Index::create(&new, Settings::default(), &small(), None, &options).unwrap();
    assert_eq!(Index::open(&old).unwrap().source(), Some("sha256:aaaa"));
    assert_eq!(
        remove_other_versions(state, "team/kb", "sha256:bbbb").unwrap(),
        1
    );
    assert!(!Index::exists(&old) && Index::exists(&new));
    assert_eq!(remove_other_versions(state, "absent", "x").unwrap(), 0);
}

#[test]
fn a_corpus_comes_from_an_overlay_or_a_folder_and_leaves_out_what_is_not_text() {
    let tree = KnowledgeTree::new()
        .with_text("kb/a.md", "hello")
        .with_text("flows/b.md", "flow")
        .with_text("docs/c.md", "no")
        .with_text("kb/t.md", "#!envsubst\nname ${AOS_OVERLAY_NAME}\n")
        .with_text("kb/r.md", "#!raw\n${AOS_OVERLAY_NAME}\n")
        .with(
            "kb/bin.dat",
            crate::KnowledgeEntry::File(vec![0xff, 0xfe, 0x00]),
        );
    let overlay = KnowledgeOverlay::new("team", Default::default(), tree);
    let corpus = Corpus::from_overlay(
        &overlay,
        &KnowledgeSettings::default(),
        &crate::CallerFacts::new(),
        &crate::Renderers::all(),
    );
    // The served text is indexed: a template as rendered, a raw file without its first line.
    assert_eq!(
        corpus.paths().collect::<Vec<_>>(),
        ["flows/b.md", "kb/a.md", "kb/r.md", "kb/t.md"]
    );
    assert_eq!(corpus.text("kb/t.md"), Some("name team\n"));
    assert_eq!(corpus.text("kb/r.md"), Some("${AOS_OVERLAY_NAME}\n"));

    let scratch = Scratch::new();
    fs::create_dir_all(scratch.0.join("root/kb")).unwrap();
    fs::create_dir_all(scratch.0.join("root/.git")).unwrap();
    fs::write(scratch.0.join("root/kb/x.md"), "text").unwrap();
    fs::write(scratch.0.join("root/kb/nul.md"), b"a\0b").unwrap();
    fs::write(scratch.0.join("root/.git/config"), "no").unwrap();
    let corpus = Corpus::from_dir(&scratch.0.join("root")).unwrap();
    assert_eq!(corpus.paths().collect::<Vec<_>>(), ["kb/x.md"]);
    assert_eq!(corpus.skipped, ["kb/nul.md"]);

    let mut corpus = Corpus::new();
    for bad in ["", "/abs", "a/../b", "a//b", "./a", "C:/x"] {
        assert!(
            matches!(corpus.insert(bad, b"x".to_vec()), Err(Error::Path(_))),
            "{bad}"
        );
    }
    corpus.insert("a\\b.md", b"x".to_vec()).unwrap();
    assert_eq!(corpus.paths().collect::<Vec<_>>(), ["a/b.md"]);
}

#[test]
fn a_corrupt_index_is_reported_not_guessed() {
    let scratch = Scratch::new();
    Index::create(
        &scratch.index(),
        Settings::default(),
        &small(),
        None,
        &Options::default(),
    )
    .unwrap();
    fs::write(
        scratch.index().join("gen-000001").join("chunks.jsonl"),
        "{\"path\":1}\n",
    )
    .unwrap();
    assert!(matches!(
        Index::open(&scratch.index()),
        Err(Error::Format(_))
    ));
    fs::write(scratch.index().join("CURRENT"), "elsewhere").unwrap();
    assert!(matches!(
        Index::open(&scratch.index()),
        Err(Error::Format(_))
    ));
}

#[test]
fn a_new_version_starts_from_the_last_one_and_embeds_only_what_differs() {
    let scratch = Scratch::new();
    let state = &scratch.0;
    let embedder = Words::new("m");
    let variant = variant_dir(&index_dir(state, "team", "sha256:aaaa"), Some("m"));
    Index::refresh(
        &variant,
        Settings::default(),
        &small(),
        Some(&embedder),
        &Options::default(),
        None,
    )
    .unwrap();
    assert_eq!(embedder.inputs.borrow().len(), 4);

    let next = index_dir(state, "team", "sha256:bbbb");
    let target = variant_dir(&next, Some("m"));
    let name = target.file_name().unwrap().to_string_lossy().into_owned();
    let seed = previous_version(state, "team", "sha256:bbbb", &name);
    assert_eq!(seed.as_deref(), Some(variant.as_path()));
    assert_eq!(previous_version(state, "team", "sha256:aaaa", &name), None);

    embedder.inputs.borrow_mut().clear();
    let mut changed = small();
    changed
        .insert("rules/style.md", b"Something else entirely.\n".to_vec())
        .unwrap();
    let report = Index::refresh(
        &target,
        Settings::default(),
        &changed,
        Some(&embedder),
        &Options::default(),
        seed.as_deref(),
    )
    .unwrap();
    assert_eq!(report.unchanged, ["kb/python.md", "kb/rust.md"]);
    assert_eq!(report.changed, ["rules/style.md"]);
    assert_eq!(embedder.inputs.borrow().len(), 1);

    // Settings that differ, or another model, start from nothing.
    let words = variant_dir(&next, None);
    let mut other = Settings::default();
    other.chunk_bytes = 200;
    embedder.inputs.borrow_mut().clear();
    let report = Index::refresh(
        &target.with_file_name("elsewhere"),
        other,
        &changed,
        Some(&embedder),
        &Options::default(),
        seed.as_deref(),
    )
    .unwrap();
    assert_eq!(report.added.len(), 3);
    assert_ne!(words, target);
}
