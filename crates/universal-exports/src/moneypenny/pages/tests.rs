//! The page store, formats and helpers, without a gateway.

use std::collections::BTreeMap;
use std::sync::Arc;

use ai6::{AssistantPageLimits, PageNetwork};

use super::*;

fn limits(json: Value) -> AssistantPageLimits {
    serde_json::from_value(json).unwrap()
}

fn page(id: &str, person: &str, files: &BTreeMap<String, Vec<u8>>) -> Page {
    Page {
        id: id.to_owned(),
        person_id: person.to_owned(),
        conversation_id: "c".to_owned(),
        kind: Kind::File,
        title: "t".to_owned(),
        entry: files.keys().next().cloned().unwrap_or_default(),
        files: files
            .iter()
            .map(|(path, bytes)| FileMeta {
                path: path.clone(),
                bytes: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            })
            .collect(),
        bytes: files.values().map(|bytes| bytes.len() as u64).sum(),
        created_at: 1.0,
        expires_at: Some(now() + 3600.0),
        exposed_by: Vec::new(),
        exposed: false,
        feeds: Vec::new(),
        shares: Vec::new(),
    }
}

fn one(name: &str, text: &str) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([(name.to_owned(), text.as_bytes().to_vec())])
}

fn store() -> (tempfile::TempDir, PageStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = PageStore::open(dir.path().join("pages")).unwrap();
    (dir, store)
}

fn keep(
    store: &PageStore,
    page: Page,
    files: &BTreeMap<String, Vec<u8>>,
    limits: &AssistantPageLimits,
) -> Result<Page, String> {
    let prepared = store.prepare(page, files, limits)?;
    store.commit(prepared).map_err(|error| error.to_string())
}

#[test]
fn page_paths_refuse_what_could_leave_the_page() {
    for good in ["index.html", "a/b.js", "img/a.png", "page.txt"] {
        assert!(valid_path(good), "{good}");
    }
    for bad in [
        "",
        "../x",
        "a/../x",
        ".git/config",
        "a/.hidden",
        ".",
        "a//b",
        "/abs",
        "a\\b",
        "a\nb",
        &"x".repeat(513),
        &format!("{}/x", "y".repeat(256)),
    ] {
        assert!(!valid_path(bad), "{bad:?}");
    }
}

#[test]
fn the_format_table_serves_text_as_text_and_the_unknown_as_a_download() {
    assert_eq!(classify("index.HTML"), Serve::Html);
    assert_eq!(classify("a.md"), Serve::Markdown);
    assert_eq!(classify("x.log"), Serve::Text("text/plain; charset=utf-8"));
    assert_eq!(classify("a.csv"), Serve::Text("text/csv; charset=utf-8"));
    assert_eq!(
        classify("app.mjs"),
        Serve::Asset("text/javascript; charset=utf-8")
    );
    assert_eq!(classify("x.wasm"), Serve::Asset("application/wasm"));
    for download in ["report.pdf", "a.exe", "noextension", "a.zip", "x.svgz"] {
        assert_eq!(classify(download), Serve::Attachment, "{download}");
    }
    for format in ["html", "markdown", "text", "csv", "json", "svg"] {
        assert!(inline_name(format).is_some(), "{format}");
    }
    assert!(inline_name("pdf").is_none());
}

#[test]
fn text_is_escaped_dates_are_utc_and_tokens_are_long_and_distinct() {
    assert_eq!(
        escape("<a href=\"x\">'&'</a>"),
        "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
    );
    assert_eq!(utc_text(0.0), "1970-01-01 00:00 UTC");
    assert_eq!(utc_text(1_798_000_000.0), "2026-12-23 04:26 UTC");
    assert_eq!(
        utc_text(951_782_400.0 + 86_400.0 * 59.0 + 3_600.0 * 13.0 + 60.0 * 5.0),
        "2000-04-28 13:05 UTC"
    );
    let (a, b) = (random_token(), random_token());
    assert_ne!(a, b);
    assert_eq!(a.len(), 43, "32 bytes as unpadded base64url");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    );
    assert_eq!(base64url(b"\xfb\xff"), "-_8");
    assert_eq!(base64url(b"Ma"), "TWE");
    assert_eq!(percent("a b/c"), "a%20b%2Fc");
}

#[test]
fn markdown_keeps_tables_and_task_lists_and_titles_are_escaped() {
    let html = markdown_page(
        "a <b>",
        "# H\n\n| a |\n|---|\n| 1 |\n\n- [x] done\n\n~~gone~~",
    );
    assert!(html.contains("<table>") && html.contains("checkbox") && html.contains("<del>"));
    assert!(html.contains("<title>a &lt;b&gt;</title>"));
}

#[test]
fn a_page_is_checked_against_every_quota_before_anything_is_stored() {
    let (_dir, store) = store();
    let defaults = AssistantPageLimits::default();
    let files = one("index.html", "<p>x</p>");
    // Files, sizes and the number of pages.
    let many: BTreeMap<String, Vec<u8>> = (0..3).map(|i| (format!("f{i}.txt"), vec![1])).collect();
    assert!(
        store
            .prepare(
                page("page-a", "p", &many),
                &many,
                &limits(json!({"max_page_files": 2}))
            )
            .unwrap_err()
            .contains("2 files")
    );
    let big = one("big.bin", &"x".repeat(100));
    assert!(
        store
            .prepare(
                page("page-a", "p", &big),
                &big,
                &limits(json!({"max_file_bytes": 50}))
            )
            .unwrap_err()
            .contains("50 bytes")
    );
    assert!(
        store
            .prepare(
                page("page-a", "p", &big),
                &big,
                &limits(json!({"max_page_bytes": 50}))
            )
            .unwrap_err()
            .contains("longer than 50")
    );
    assert!(
        store
            .prepare(
                page("page-a", "p", &BTreeMap::new()),
                &BTreeMap::new(),
                &defaults
            )
            .is_err()
    );
    let bad = BTreeMap::from([("../x".to_owned(), vec![1])]);
    assert!(
        store
            .prepare(page("page-a", "p", &bad), &bad, &defaults)
            .unwrap_err()
            .contains("not a path")
    );
    // Nothing was written by a refusal.
    assert_eq!(std::fs::read_dir(store.dir.clone()).unwrap().count(), 0);

    let tight = limits(json!({"max_pages": 2, "max_person_bytes": 20, "max_total_bytes": 30}));
    keep(&store, page("page-1", "p", &files), &files, &tight).unwrap();
    // The same ID replaces itself: its own size does not count against it.
    keep(&store, page("page-1", "p", &files), &files, &tight).unwrap();
    keep(&store, page("page-2", "p", &files), &files, &tight).unwrap();
    assert!(
        store
            .prepare(page("page-3", "p", &files), &files, &tight)
            .unwrap_err()
            .contains("2 pages")
    );
    // Another person has their own pages and person bytes, under the gateway's total.
    keep(
        &store,
        page("page-4", "q", &files),
        &files,
        &limits(json!({"max_total_bytes": 100})),
    )
    .unwrap();
    assert!(
        store
            .prepare(
                page("page-5", "q", &files),
                &files,
                &limits(json!({"max_person_bytes": 10}))
            )
            .unwrap_err()
            .contains("10 bytes in all")
    );
    assert!(
        store
            .prepare(
                page("page-5", "r", &files),
                &files,
                &limits(json!({"max_total_bytes": 20}))
            )
            .unwrap_err()
            .contains("all the page bytes")
    );
}

#[test]
fn a_discarded_page_leaves_nothing_and_a_committed_one_survives_a_restart() {
    let (dir, store) = store();
    let defaults = AssistantPageLimits::default();
    let files = one("index.html", "<p>kept</p>");
    let prepared = store
        .prepare(page("page-gone", "p", &files), &files, &defaults)
        .unwrap();
    store.discard(prepared);
    assert!(store.get("page-gone").is_none());
    assert_eq!(
        std::fs::read_dir(dir.path().join("pages")).unwrap().count(),
        0
    );

    keep(&store, page("page-kept", "p", &files), &files, &defaults).unwrap();
    store
        .remember("page-kept", true, Duration::from_secs(60))
        .unwrap();
    keep(&store, page("page-temp", "p", &files), &files, &defaults).unwrap();
    // A publish that never finished is cleaned at start.
    std::fs::create_dir_all(
        dir.path()
            .join("pages")
            .join("page-half.tmp-1")
            .join("files"),
    )
    .unwrap();
    drop(store);
    let reopened = PageStore::open(dir.path().join("pages")).unwrap();
    let kept = reopened.get("page-kept").unwrap();
    assert_eq!(kept.expires_at, None, "remembered stays remembered");
    assert_eq!(
        reopened.read_file(&kept, "index.html").unwrap(),
        b"<p>kept</p>"
    );
    assert!(reopened.get("page-temp").is_some());
    assert!(!dir.path().join("pages").join("page-half.tmp-1").exists());
    // The sweeper deletes what has expired and keeps what is remembered.
    let gone = reopened.sweep(now() + 7200.0, Duration::from_secs(300));
    assert_eq!(
        gone.iter().map(|page| page.id.as_str()).collect::<Vec<_>>(),
        ["page-temp"]
    );
    assert!(reopened.get("page-kept").is_some());
    assert!(!dir.path().join("pages").join("page-temp").exists());
    // Only listed files are read, whatever path is asked.
    assert!(
        reopened
            .read_file(&kept, "../page-temp/meta.json")
            .is_none()
    );
    assert!(reopened.read_file(&kept, "meta.json").is_none());
}

#[test]
fn links_stand_for_their_page_and_time_and_are_counted() {
    let (_dir, store) = store();
    let files = one("index.html", "x");
    let made = keep(
        &store,
        page("page-1", "p", &files),
        &files,
        &AssistantPageLimits::default(),
    )
    .unwrap();
    let (token, expires) = store.mint(
        &made,
        "p",
        "bearer",
        "digest",
        PageNetwork::Own,
        Duration::from_secs(3600),
    );
    assert!(expires <= made.expires_at.unwrap());
    let (link, _) = store.link(&token).unwrap();
    assert_eq!(
        (link.viewer.as_str(), link.network),
        ("p", PageNetwork::Own)
    );
    assert!(store.link("not-the-token").is_none());
    assert!(store.first_open(&token));
    assert!(!store.first_open(&token));
    for _ in 0..3 {
        assert!(store.note_read(&token, 3));
    }
    assert!(!store.note_read(&token, 3), "the fourth read in a minute");
    let (short, _) = store.mint(&made, "p", "b", "d", PageNetwork::Open, Duration::ZERO);
    assert!(
        store.link(&short).is_none(),
        "a link of no time stands for none"
    );
    store.delete("page-1");
    assert!(store.link(&token).is_none(), "the page's links go with it");
}

#[test]
fn a_station_upload_is_refused_for_bad_paths_sizes_counts_and_another_station() {
    let (_dir, store) = store();
    let defaults = AssistantPageLimits::default();
    for path in ["../x", ".git/config", "a//b", ""] {
        assert!(
            store
                .stage_file("hq", "c", "d", path, vec![1], &defaults)
                .is_err(),
            "{path:?}"
        );
    }
    // The default file quota is 8 MiB: 9 MiB is refused.
    assert!(
        store
            .stage_file(
                "hq",
                "c",
                "d",
                "big.bin",
                vec![0; 9 * 1024 * 1024],
                &defaults
            )
            .is_err()
    );
    store
        .stage_file("hq", "c", "d", "a.txt", vec![1], &defaults)
        .unwrap();
    assert!(
        store
            .stage_file("worker", "c", "d", "b.txt", vec![1], &defaults)
            .is_err(),
        "another Station"
    );
    assert!(
        store
            .stage_done("worker", "c", "d", Ok(Vec::new()), &defaults)
            .is_err()
    );
    let few = limits(json!({"max_page_files": 1, "max_page_bytes": 3}));
    assert!(
        store
            .stage_file("hq", "c", "d", "b.txt", vec![1], &few)
            .is_err(),
        "a second file"
    );
    assert!(
        store
            .stage_file("hq", "c", "d", "a.txt", vec![1; 4], &few)
            .is_err(),
        "the page's bytes"
    );
    let one = limits(json!({"staged_uploads": 1}));
    assert!(
        store
            .stage_file("hq", "c2", "d2", "a.txt", vec![1], &one)
            .is_err(),
        "too many wait"
    );
    // A finished upload takes no more files.
    store
        .stage_done("hq", "c", "d", Ok(Vec::new()), &defaults)
        .unwrap();
    assert!(
        store
            .stage_file("hq", "c", "d", "c.txt", vec![1], &defaults)
            .is_err()
    );
}

#[tokio::test]
async fn a_claim_waits_for_the_station_checks_the_manifest_and_gives_up() {
    let (_dir, store) = store();
    let store = Arc::new(store);
    let defaults = AssistantPageLimits::default();
    let bytes = b"<p>x</p>".to_vec();
    let file = AssistantPageFile {
        path: "index.html".to_owned(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
        outside_text: true,
    };
    // Files that arrive while the claim waits.
    let waiting = {
        let store = store.clone();
        tokio::spawn(async move { store.claim("c", "d", Duration::from_secs(5)).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    store
        .stage_file("hq", "c", "d", "index.html", bytes.clone(), &defaults)
        .unwrap();
    store
        .stage_done("hq", "c", "d", Ok(vec![file.clone()]), &defaults)
        .unwrap();
    let (files, manifest) = waiting.await.unwrap().unwrap();
    assert_eq!(files["index.html"], bytes);
    assert!(manifest[0].outside_text);
    // An upload is taken once.
    assert!(
        store
            .claim("c", "d", Duration::from_millis(100))
            .await
            .is_err()
    );
    // A manifest naming a file that never came, one with another digest, one with another size.
    for lie in [
        AssistantPageFile {
            path: "other.html".to_owned(),
            ..file.clone()
        },
        AssistantPageFile {
            sha256: "00".to_owned(),
            ..file.clone()
        },
        AssistantPageFile {
            bytes: 1,
            ..file.clone()
        },
    ] {
        store
            .stage_file("hq", "c", "e", "index.html", bytes.clone(), &defaults)
            .unwrap();
        store
            .stage_done("hq", "c", "e", Ok(vec![lie]), &defaults)
            .unwrap();
        let error = store
            .claim("c", "e", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.contains("manifest"), "{error}");
    }
    // The Station's own refusal, and silence.
    store
        .stage_done("hq", "c", "f", Err("a link".to_owned()), &defaults)
        .unwrap();
    assert!(
        store
            .claim("c", "f", Duration::from_secs(1))
            .await
            .unwrap_err()
            .contains("a link")
    );
    let silent = store
        .claim("c", "nothing", Duration::from_millis(150))
        .await
        .unwrap_err();
    assert!(silent.contains("sent no files within"), "{silent}");
    // Stale uploads go with the sweeper.
    store
        .stage_file("hq", "c", "stale", "a.txt", vec![1], &defaults)
        .unwrap();
    store.sweep(now(), Duration::ZERO);
    assert!(
        store
            .claim("c", "stale", Duration::from_millis(100))
            .await
            .is_err()
    );
}

#[test]
fn feeds_are_read_tools_with_valid_input_distinct_names_and_a_cap() {
    let defaults = AssistantPageLimits::default();
    let feed = |name: &str, tool: &str, input: Value| Feed {
        name: name.to_owned(),
        tool: tool.to_owned(),
        input,
    };
    assert!(check_feeds(&[feed("board", "board", json!({}))], &defaults).is_ok());
    assert!(
        check_feeds(
            &[feed(
                "m",
                "mission",
                json!({"operation_id": "G", "mission_id": "m"})
            )],
            &defaults
        )
        .is_ok()
    );
    for refused in [
        feed("a", "propose_mission", json!({})),
        feed("a", "propose_shared_change", json!({})),
        feed("a", "send_cable", json!({})),
        feed("a", "show", json!({})),
        feed("a", "publish_page", json!({})),
        feed("a", "pages", json!({})),
        feed("a", "unpublish_page", json!({})),
        feed("a", "field_guide", json!({})),
        feed("a", "nonsense", json!({})),
        feed("a", "mission", json!({"operation_id": "G"})),
        feed("", "board", json!({})),
        feed("a b", "board", json!({})),
        feed("../a", "board", json!({})),
    ] {
        assert!(
            check_feeds(std::slice::from_ref(&refused), &defaults).is_err(),
            "{refused:?}"
        );
    }
    assert!(
        check_feeds(
            &[feed("a", "board", json!({})), feed("a", "board", json!({}))],
            &defaults
        )
        .is_err(),
        "names are distinct"
    );
    let two = limits(json!({"max_feeds": 1}));
    assert!(
        check_feeds(
            &[feed("a", "board", json!({})), feed("b", "board", json!({}))],
            &two
        )
        .is_err()
    );
    assert!(
        check_feeds(
            &[feed("a", "board", json!({}))],
            &limits(json!({"feeds": false}))
        )
        .is_err()
    );
    assert_eq!(profile(&defaults, None), PageNetwork::Open);
    assert_eq!(
        profile(&defaults, Some(PageNetwork::None)),
        PageNetwork::None
    );
    assert_eq!(
        profile(&limits(json!({"network": "own"})), Some(PageNetwork::Open)),
        PageNetwork::Own
    );
}
