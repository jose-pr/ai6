use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::directorates::q::sealing::{MARKS_FILE, Marks, SealingKey};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::StationSealingKey;
use crate::directorates::registry::store::FileRegistry;

const NOW: f64 = 1_790_000_000.0;
const PLANTED: &str = "planted-secret-0451";

fn registry(dir: &Path) -> Arc<dyn RegistryBackend> {
    Arc::new(FileRegistry::open(dir))
}

/// A Station with its sealing key published in `registry`.
fn station(registry: &Arc<dyn RegistryBackend>, name: &str) -> Vault {
    let key = Arc::new(SealingKey::generate());
    registry
        .publish_sealing_key(&StationSealingKey::new(name, key.public_key().to_text()))
        .unwrap();
    Vault::new(registry.clone(), name, key)
}

fn named(stations: &[&str]) -> Vec<Recipient> {
    stations
        .iter()
        .map(|name| Recipient::named(*name))
        .collect()
}

fn op() -> Scope {
    Scope::operation("alpha").unwrap()
}

/// Two Stations in one scope, `a` and `b`, and `c` outside it.
fn fleet(dir: &Path) -> (Arc<dyn RegistryBackend>, Vault, Vault, Vault) {
    let registry = registry(dir);
    let (a, b, c) = (
        station(&registry, "a"),
        station(&registry, "b"),
        station(&registry, "c"),
    );
    a.put_scope_key(&op(), &named(&["a", "b"]), NOW).unwrap();
    (registry, a, b, c)
}

#[test]
fn one_write_is_read_by_every_station_the_key_is_wrapped_for() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, b, _c) = fleet(dir.path());
    let revision = a
        .write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    assert_eq!(revision, 1);
    for vault in [&a, &b] {
        let opened = vault.read_value("forge").unwrap();
        assert_eq!(opened.value.as_str(), PLANTED);
        assert_eq!(opened.revision, 1);
        assert_eq!(opened.scope, op());
    }
    // One more write, from the other Station, reaches the first with no copy made.
    b.write_value("forge", &op(), "second", SealedExpect::Revision(1), NOW)
        .unwrap();
    assert_eq!(a.read_value("forge").unwrap().value.as_str(), "second");
    assert!(matches!(a.read_value("nothing"), Err(VaultError::NotFound)));
}

#[test]
fn a_station_without_a_wrap_is_refused_to_read_and_to_write() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, _b, c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    assert!(matches!(c.read_value("forge"), Err(VaultError::NotNamed)));
    assert!(matches!(
        c.write_value("forge", &op(), "x", SealedExpect::Any, NOW),
        Err(VaultError::NotNamed)
    ));
    assert!(matches!(
        c.grant_station(&op(), &Recipient::named("c"), NOW),
        Err(VaultError::NotNamed)
    ));
    assert!(matches!(
        c.revoke_station(&op(), "a", NOW),
        Err(VaultError::NotNamed)
    ));
}

#[test]
fn a_wrap_written_into_the_record_by_hand_opens_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, _b, c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    // Copy another Station's wrap under c's name: it was sealed to a different key.
    let mut record = registry.sealed_value(&key_name(&op())).unwrap().unwrap();
    let wrap = record.copies["w:a"].clone();
    record.copies.insert("w:c".to_owned(), wrap);
    registry
        .store_sealed(
            &key_name(&op()),
            &record.copies,
            "a",
            SealedExpect::Any,
            NOW,
        )
        .unwrap();
    assert!(matches!(c.read_value("forge"), Err(VaultError::Unreadable)));
}

#[test]
fn a_granted_station_reads_what_was_written_before_it_was_granted() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, _b, c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    let info = a.grant_station(&op(), &Recipient::named("c"), NOW).unwrap();
    assert!(info.stations.contains("c"));
    assert!(!info.rotating);
    assert_eq!(c.read_value("forge").unwrap().value.as_str(), PLANTED);
    // A grant leaves the value and its key alone.
    assert_eq!(c.read_value("forge").unwrap().revision, 1);
    assert!(matches!(
        a.grant_station(&op(), &Recipient::named("nobody"), NOW),
        Err(VaultError::NoKey(_))
    ));
}

#[test]
fn a_revoked_station_cannot_read_the_new_revision_nor_use_its_old_wrap() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    a.write_value("forge", &op(), "before", SealedExpect::Absent, NOW)
        .unwrap();
    let before = registry.sealed_value(&key_name(&op())).unwrap().unwrap();
    let info = a.revoke_station(&op(), "b", NOW).unwrap();
    assert_eq!(info.stations, BTreeSet::from(["a".to_owned()]));
    assert!(!info.rotating);
    a.write_value("forge", &op(), PLANTED, SealedExpect::Any, NOW)
        .unwrap();
    assert_eq!(a.read_value("forge").unwrap().value.as_str(), PLANTED);
    assert!(matches!(b.read_value("forge"), Err(VaultError::NotNamed)));
    // Even with the Registry's old key record put back, the old wrap opens the old key, and that
    // key no longer opens what is written now.
    let old_key = b.open_wrap(&op(), &before.copies["w:b"]).unwrap();
    let now = registry
        .sealed_value(&value_name("forge"))
        .unwrap()
        .unwrap();
    let stored = StoredValue::parse(&now).unwrap();
    assert!(decrypt(&old_key, "forge", &stored).is_err());
}

#[test]
fn rotating_the_key_leaves_every_value_readable_by_the_stations_that_remain() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    for (name, value) in [("one", "1"), ("two", "2"), ("three", "3")] {
        a.write_value(name, &op(), value, SealedExpect::Absent, NOW)
            .unwrap();
    }
    let other = Scope::global();
    a.put_scope_key(&other, &named(&["a"]), NOW).unwrap();
    a.write_value("elsewhere", &other, "g", SealedExpect::Absent, NOW)
        .unwrap();
    let before = a.scope_key_info(&op()).unwrap().unwrap();
    let after = a.rotate_scope_key(&op(), NOW).unwrap();
    assert_ne!(before.key_id, after.key_id);
    assert!(after.revision > before.revision);
    for vault in [&a, &b] {
        for (name, value) in [("one", "1"), ("two", "2"), ("three", "3")] {
            let opened = vault.read_value(name).unwrap();
            assert_eq!(opened.value.as_str(), value);
            assert_eq!(opened.key_id, after.key_id);
            assert_eq!(opened.revision, 2, "a re-encryption is a new revision");
        }
    }
    // Another scope is untouched.
    let untouched = registry
        .sealed_value(&value_name("elsewhere"))
        .unwrap()
        .unwrap();
    assert_eq!(untouched.revision, 1);
}

#[test]
fn a_rotation_a_crash_left_open_is_readable_and_finished_by_a_remaining_station() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, b, _c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    // Only step one: the new key is current, no value moved.
    let (next, _old, _new) = a.begin_rotation(&op(), Some("a"), NOW).unwrap();
    assert!(next.rotating());
    let info = a.scope_key_info(&op()).unwrap().unwrap();
    assert!(info.rotating);
    assert_eq!(info.stations, BTreeSet::from(["b".to_owned()]));
    // The Station that was dropped is shut out at once; the one that remains still reads.
    assert!(matches!(a.read_value("forge"), Err(VaultError::NotNamed)));
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
    b.finish_rotation(&op(), NOW).unwrap();
    let info = b.scope_key_info(&op()).unwrap().unwrap();
    assert!(!info.rotating);
    let opened = b.read_value("forge").unwrap();
    assert_eq!(opened.value.as_str(), PLANTED);
    assert_eq!(opened.key_id, info.key_id);
}

#[test]
fn the_last_station_of_a_scope_cannot_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, _b, _c) = fleet(dir.path());
    a.revoke_station(&op(), "b", NOW).unwrap();
    assert!(matches!(
        a.revoke_station(&op(), "a", NOW),
        Err(VaultError::LastStation)
    ));
    assert!(matches!(
        a.revoke_station(&op(), "ghost", NOW),
        Err(VaultError::NotNamed)
    ));
}

#[test]
fn a_tampered_ciphertext_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    let record = registry
        .sealed_value(&value_name("forge"))
        .unwrap()
        .unwrap();
    // One flipped bit.
    let mut copies = record.copies.clone();
    let copy = copies.get_mut("operation:alpha").unwrap();
    let mut bytes = from_base64(&copy.sealed).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    copy.sealed = base64_text(&bytes);
    registry
        .store_sealed(
            &value_name("forge"),
            &copies,
            "x",
            SealedExpect::Revision(1),
            NOW,
        )
        .unwrap();
    assert!(matches!(b.read_value("forge"), Err(VaultError::Unreadable)));
}

#[test]
fn a_ciphertext_moved_to_another_name_scope_or_revision_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    let global = Scope::global();
    a.put_scope_key(&global, &named(&["a", "b"]), NOW).unwrap();
    a.write_value("one", &op(), "first", SealedExpect::Absent, NOW)
        .unwrap();
    a.write_value("two", &op(), "second", SealedExpect::Absent, NOW)
        .unwrap();
    let one = registry.sealed_value(&value_name("one")).unwrap().unwrap();

    // Another name.
    registry
        .store_sealed(&value_name("two"), &one.copies, "x", SealedExpect::Any, NOW)
        .unwrap();
    assert!(matches!(b.read_value("two"), Err(VaultError::Unreadable)));

    // Another scope, naming that scope's key ID so only the binding stops it.
    let global_id = a.scope_key_info(&global).unwrap().unwrap().key_id;
    let mut copies = BTreeMap::new();
    copies.insert(
        global.to_string(),
        SealedCopy {
            key: global_id,
            sealed: one.copies["operation:alpha"].sealed.clone(),
        },
    );
    registry
        .store_sealed(&value_name("one"), &copies, "x", SealedExpect::Any, NOW)
        .unwrap();
    assert!(matches!(b.read_value("one"), Err(VaultError::Unreadable)));

    // An older revision's ciphertext put back after a newer write.
    a.write_value("three", &op(), "v1", SealedExpect::Absent, NOW)
        .unwrap();
    let v1 = registry
        .sealed_value(&value_name("three"))
        .unwrap()
        .unwrap();
    a.write_value("three", &op(), "v2", SealedExpect::Revision(1), NOW)
        .unwrap();
    registry
        .store_sealed(
            &value_name("three"),
            &v1.copies,
            "x",
            SealedExpect::Any,
            NOW,
        )
        .unwrap();
    assert!(matches!(b.read_value("three"), Err(VaultError::Unreadable)));
}

#[test]
fn a_wrap_moved_to_another_scope_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, _b, _c) = fleet(dir.path());
    let global = Scope::global();
    a.put_scope_key(&global, &named(&["a"]), NOW).unwrap();
    a.write_value("forge", &global, PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    // The operation's wrap for a, under the global scope's record.
    let operation = registry.sealed_value(&key_name(&op())).unwrap().unwrap();
    let mut forged = registry
        .sealed_value(&key_name(&global))
        .unwrap()
        .unwrap()
        .copies;
    forged.insert("w:a".to_owned(), operation.copies["w:a"].clone());
    registry
        .store_sealed(&key_name(&global), &forged, "x", SealedExpect::Any, NOW)
        .unwrap();
    assert!(matches!(a.read_value("forge"), Err(VaultError::Unreadable)));
}

#[test]
fn a_value_stays_in_its_scope_and_a_stale_expectation_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, _b, _c) = fleet(dir.path());
    let global = Scope::global();
    a.put_scope_key(&global, &named(&["a"]), NOW).unwrap();
    a.write_value("forge", &op(), "one", SealedExpect::Absent, NOW)
        .unwrap();
    assert!(matches!(
        a.write_value("forge", &global, "two", SealedExpect::Any, NOW),
        Err(VaultError::WrongScope)
    ));
    assert!(matches!(
        a.write_value("forge", &op(), "two", SealedExpect::Absent, NOW),
        Err(VaultError::Conflict { held: Some(1) })
    ));
    assert!(matches!(
        a.write_value("forge", &op(), "two", SealedExpect::Revision(7), NOW),
        Err(VaultError::Conflict { held: Some(1) })
    ));
    assert!(matches!(
        a.write_value(
            "new",
            &Scope::compartment("c1").unwrap(),
            "x",
            SealedExpect::Any,
            NOW
        ),
        Err(VaultError::NoScopeKey(_))
    ));
    assert!(matches!(
        a.put_scope_key(&op(), &named(&["a"]), NOW),
        Err(VaultError::Conflict { .. })
    ));
}

#[test]
fn scopes_are_global_operation_or_compartment_and_nothing_else() {
    assert_eq!(Scope::parse("global").unwrap(), Scope::global());
    assert_eq!(
        Scope::parse("operation:x").unwrap().owner(),
        Some(("operation", "x"))
    );
    for bad in [
        "",
        "operation:",
        "team:x",
        "global:x",
        "operation:a\nb",
        "Global",
    ] {
        assert!(
            matches!(Scope::parse(bad), Err(VaultError::BadScope(_))),
            "{bad:?}"
        );
    }
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn a_planted_value_and_key_appear_in_no_record_debug_or_error() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    let mut seen = String::new();
    for entry in walk(dir.path()) {
        seen.push_str(&String::from_utf8_lossy(&std::fs::read(entry).unwrap()));
    }
    for name in [key_name(&op()), value_name("forge")] {
        let record = registry.sealed_value(&name).unwrap().unwrap();
        seen.push_str(&format!("{record:?}"));
        seen.push_str(&serde_json::to_string(&record).unwrap());
    }
    let opened = a.read_value("forge").unwrap();
    seen.push_str(&format!("{opened:?} {a:?}"));
    seen.push_str(&format!("{:?}", a.scope_key_info(&op()).unwrap()));
    seen.push_str(&format!("{:?}", a.value_infos().unwrap()));
    for error in [
        c.read_value("forge").unwrap_err(),
        a.read_value("none").unwrap_err(),
        b.write_value("x", &Scope::global(), PLANTED, SealedExpect::Any, NOW)
            .unwrap_err(),
    ] {
        seen.push_str(&error.to_string());
        seen.push_str(&format!("{error:?}"));
    }
    // The key itself: its raw bytes in any spelling.
    let stored = a.load_key(&op()).unwrap();
    let key = a.current_key(&op(), &stored).unwrap();
    seen.push_str(&format!("{key:?}"));
    for spelling in [
        base64_text(key.key.as_ref()),
        hex(key.key.as_ref()),
        String::from_utf8_lossy(key.key.as_ref()).into_owned(),
    ] {
        assert!(!seen.contains(&spelling), "the key leaked");
    }
    assert_eq!(opened.value.as_str(), PLANTED, "it was there to be found");
    assert!(!seen.contains(PLANTED), "{seen}");
    assert!(!seen.contains(&base64_text(PLANTED.as_bytes())), "{seen}");
}

#[test]
fn values_are_listed_without_being_opened() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, _b, _c) = fleet(dir.path());
    a.write_value("one", &op(), "1", SealedExpect::Absent, NOW)
        .unwrap();
    a.write_value("two", &op(), "2", SealedExpect::Absent, NOW)
        .unwrap();
    let infos = a.value_infos().unwrap();
    assert_eq!(
        infos
            .iter()
            .map(|info| info.name.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert!(a.remove_value_in("one", &op()).unwrap());
    assert!(a.value_info("one").unwrap().is_none());
}

use crate::directorates::registry::suite::{Store, backend_tests};

backend_tests!(
    a_scope_key_serves_every_station_and_a_revoke_shuts_one_out,
    a_rotation_re_encrypts_every_value_of_the_scope,
    a_request_is_answered_and_a_value_moves_off_its_per_station_seal_on_every_backend,
);

/// A fleet of `a`, `b` and `c` on `store`'s Registry, each its own handle.
fn fleet_on(store: &dyn Store) -> (Vault, Vault, Vault) {
    let make = |name: &str| {
        let registry: Arc<dyn RegistryBackend> = store.handle();
        station(&registry, name)
    };
    (make("a"), make("b"), make("c"))
}

fn a_scope_key_serves_every_station_and_a_revoke_shuts_one_out(store: &dyn Store) {
    let (a, b, c) = fleet_on(store);
    a.put_scope_key(&op(), &named(&["a", "b"]), NOW).unwrap();
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
    assert!(matches!(c.read_value("forge"), Err(VaultError::NotNamed)));
    a.grant_station(&op(), &Recipient::named("c"), NOW).unwrap();
    assert_eq!(c.read_value("forge").unwrap().value.as_str(), PLANTED);
    a.revoke_station(&op(), "b", NOW).unwrap();
    a.write_value("forge", &op(), "later", SealedExpect::Any, NOW)
        .unwrap();
    assert_eq!(c.read_value("forge").unwrap().value.as_str(), "later");
    assert!(matches!(b.read_value("forge"), Err(VaultError::NotNamed)));
}

fn a_rotation_re_encrypts_every_value_of_the_scope(store: &dyn Store) {
    let (a, b, _c) = fleet_on(store);
    a.put_scope_key(&op(), &named(&["a", "b"]), NOW).unwrap();
    for name in ["one", "two"] {
        a.write_value(name, &op(), name, SealedExpect::Absent, NOW)
            .unwrap();
    }
    let before = a.scope_key_info(&op()).unwrap().unwrap();
    let after = a.rotate_scope_key(&op(), NOW).unwrap();
    assert_ne!(before.key_id, after.key_id);
    for name in ["one", "two"] {
        let opened = b.read_value(name).unwrap();
        assert_eq!(opened.value.as_str(), name);
        assert_eq!(opened.key_id, after.key_id);
    }
}

// Regression tests from the security review: each was an attack that worked and now fails.

/// What the review's first probe did: a Station that was revoked puts the key record from before
/// the revoke back (the Registry gives it a newer revision), hoping the next write is encrypted
/// under the key it still holds.
#[test]
fn a_key_record_put_back_after_a_revoke_is_refused_and_the_next_write_is_not_under_the_old_key() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    a.write_value("forge", &op(), "before", SealedExpect::Absent, NOW)
        .unwrap();
    let before = registry.sealed_value(&key_name(&op())).unwrap().unwrap();
    let old_key = b
        .current_key(&op(), &StoredKey::parse(&before).unwrap())
        .unwrap();
    a.revoke_station(&op(), "b", NOW).unwrap();
    registry
        .store_sealed(
            &key_name(&op()),
            &before.copies,
            "b",
            SealedExpect::Any,
            NOW,
        )
        .unwrap();
    let refused = a
        .write_value("forge", &op(), PLANTED, SealedExpect::Any, NOW)
        .unwrap_err();
    assert!(matches!(refused, VaultError::Rollback(_)), "{refused}");
    let held = registry
        .sealed_value(&value_name("forge"))
        .unwrap()
        .unwrap();
    let stored = StoredValue::parse(&held).unwrap();
    assert!(decrypt(&old_key, "forge", &stored).is_err());
    assert!(matches!(
        a.read_value("forge"),
        Err(VaultError::Rollback(_))
    ));
}

/// The review's fifth probe: a value removed from the Registry and put back as it was restarts
/// its revisions at 1; a Station that accepted a later one refuses it.
#[test]
fn a_value_removed_and_put_back_is_refused_by_a_station_that_saw_a_later_one() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    a.write_value("token", &op(), "old-leaked", SealedExpect::Absent, NOW)
        .unwrap();
    let v1 = registry
        .sealed_value(&value_name("token"))
        .unwrap()
        .unwrap();
    a.write_value("token", &op(), "new", SealedExpect::Revision(1), NOW)
        .unwrap();
    assert_eq!(b.read_value("token").unwrap().value.as_str(), "new");
    registry.remove_sealed(&value_name("token")).unwrap();
    registry
        .store_sealed(
            &value_name("token"),
            &v1.copies,
            "c",
            SealedExpect::Absent,
            NOW,
        )
        .unwrap();
    assert!(matches!(
        b.read_value("token"),
        Err(VaultError::Rollback(_))
    ));
}

#[test]
fn a_removed_value_leaves_a_tombstone_and_its_revisions_go_on() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    a.write_value("token", &op(), "one", SealedExpect::Absent, NOW)
        .unwrap();
    assert!(a.remove_value_in("token", &op()).unwrap());
    assert!(!a.remove_value_in("token", &op()).unwrap(), "already gone");
    assert!(matches!(b.read_value("token"), Err(VaultError::NotFound)));
    assert!(a.value_info("token").unwrap().is_none());
    assert!(a.value_infos().unwrap().is_empty());
    assert!(
        registry
            .sealed_value(&value_name("token"))
            .unwrap()
            .is_some()
    );
    // A name that was removed takes a new value as a new revision, never revision 1 again.
    assert!(matches!(
        a.write_value("token", &op(), "x", SealedExpect::Revision(1), NOW),
        Err(VaultError::Conflict { .. })
    ));
    let revision = a
        .write_value("token", &op(), "two", SealedExpect::Absent, NOW)
        .unwrap();
    assert_eq!(revision, 3);
    assert_eq!(b.read_value("token").unwrap().value.as_str(), "two");
    assert!(matches!(
        a.write_value("token", &op(), "three", SealedExpect::Absent, NOW),
        Err(VaultError::Conflict { held: Some(3) })
    ));
}

/// The review's fourth probe: a write that opened the key before a revoke and stored after it.
#[test]
fn a_write_racing_a_revoke_is_encrypted_again_under_the_new_key() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    let old = a.current_key(&op(), &a.load_key(&op()).unwrap()).unwrap();
    a.write_value_between("late", &op(), PLANTED, SealedExpect::Absent, NOW, &|| {
        a.revoke_station(&op(), "b", NOW).unwrap();
    })
    .unwrap();
    let info = a.scope_key_info(&op()).unwrap().unwrap();
    let opened = a.read_value("late").unwrap();
    assert_eq!(opened.value.as_str(), PLANTED);
    assert_eq!(opened.key_id, info.key_id);
    let held = registry.sealed_value(&value_name("late")).unwrap().unwrap();
    assert!(decrypt(&old, "late", &StoredValue::parse(&held).unwrap()).is_err());
    assert!(matches!(b.read_value("late"), Err(VaultError::NotNamed)));
}

#[test]
fn a_writer_dropped_while_it_wrote_leaves_no_value_under_a_key_only_it_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, b, _c) = fleet(dir.path());
    let refused = b
        .write_value_between("late", &op(), PLANTED, SealedExpect::Absent, NOW, &|| {
            a.revoke_station(&op(), "b", NOW).unwrap();
        })
        .unwrap_err();
    assert!(matches!(refused, VaultError::NotNamed), "{refused}");
    assert!(matches!(a.read_value("late"), Err(VaultError::NotFound)));
}

/// The review's third probe: only the unauthenticated `key` field of a wrap is changed, to
/// another Station's key; the next rotation used to wrap to it.
#[test]
fn a_wrap_whose_public_key_was_changed_is_refused_and_does_not_redirect_a_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    let mut record = registry.sealed_value(&key_name(&op())).unwrap().unwrap();
    record.copies.get_mut("w:b").unwrap().key = c.key.public_key().to_text();
    registry
        .store_sealed(
            &key_name(&op()),
            &record.copies,
            "c",
            SealedExpect::Any,
            NOW,
        )
        .unwrap();
    assert!(matches!(b.read_value("forge"), Err(VaultError::Unreadable)));
    a.rotate_scope_key(&op(), NOW).unwrap();
    let keys =
        StoredKey::parse(&registry.sealed_value(&key_name(&op())).unwrap().unwrap()).unwrap();
    assert!(c.open_wrap(&op(), &keys.current["b"]).is_err());
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
}

/// A key published under a Station's name by another is wrapped to only when it verifies against
/// the pin the grant recorded.
#[cfg(feature = "station-tls")]
#[test]
fn a_rotation_wraps_only_to_a_key_that_verifies_against_the_recorded_pin() {
    use crate::station::identity::{open_identity, sign_sealing_key};
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path());
    let (a, c) = (station(&registry, "a"), station(&registry, "c"));
    let identity_dir = tempfile::tempdir().unwrap();
    let identity = open_identity(identity_dir.path()).unwrap();
    let fingerprint = identity.fingerprint().to_string();
    let publish_b = |key: &SealingKey| {
        let signed = sign_sealing_key(&identity, "b", &key.public_key().to_text()).unwrap();
        registry.publish_sealing_key(&signed).unwrap();
    };
    let b_key = SealingKey::generate();
    publish_b(&b_key);
    a.put_scope_key(
        &op(),
        &[
            Recipient::named("a"),
            Recipient::named("b").pinned(&fingerprint),
        ],
        NOW,
    )
    .unwrap();
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    assert_eq!(a.load_key(&op()).unwrap().pins["b"], fingerprint);
    // Another Station publishes its own key under b's name.
    registry
        .publish_sealing_key(&StationSealingKey::new("b", c.key.public_key().to_text()))
        .unwrap();
    let before = a.scope_key_info(&op()).unwrap().unwrap();
    let refused = a.rotate_scope_key(&op(), NOW).unwrap_err();
    assert!(
        matches!(&refused, VaultError::Unverified(station) if station == "b"),
        "{refused}"
    );
    assert_eq!(a.scope_key_info(&op()).unwrap().unwrap(), before);
    // b changes its key, signed by the same identity: the next rotation wraps to the new one.
    let new_key = Arc::new(SealingKey::generate());
    publish_b(&new_key);
    a.rotate_scope_key(&op(), NOW).unwrap();
    let b = Vault::new(registry.clone(), "b", new_key);
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
    assert!(c.read_value("forge").is_err());
}

/// The review's eighth probe: a rotation over one that cannot be finished.
#[test]
fn a_rotation_or_grant_over_an_unfinished_one_this_station_cannot_finish_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, _b, _c) = fleet(dir.path());
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    a.begin_rotation(&op(), None, NOW).unwrap();
    let mut record = registry.sealed_value(&key_name(&op())).unwrap().unwrap();
    record.copies.retain(|label, _| !label.starts_with("p:"));
    registry
        .store_sealed(
            &key_name(&op()),
            &record.copies,
            "x",
            SealedExpect::Any,
            NOW,
        )
        .unwrap();
    let before = a.scope_key_info(&op()).unwrap().unwrap();
    assert!(matches!(
        a.finish_rotation(&op(), NOW),
        Err(VaultError::Rotating(_))
    ));
    assert!(matches!(
        a.rotate_scope_key(&op(), NOW),
        Err(VaultError::Rotating(_))
    ));
    assert!(matches!(
        a.grant_station(&op(), &Recipient::named("c"), NOW),
        Err(VaultError::Rotating(_))
    ));
    assert_eq!(a.scope_key_info(&op()).unwrap().unwrap(), before);
}

#[test]
fn a_plaintext_that_is_not_text_is_refused_without_holding_it_in_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, b, _c) = fleet(dir.path());
    let stored = a.load_key(&op()).unwrap();
    let key = a.current_key(&op(), &stored).unwrap();
    a.store_encrypted(
        &key,
        "bin",
        &op(),
        &[0xff, 0xfe, 0x41],
        SealedExpect::Absent,
        NOW,
    )
    .unwrap();
    let error = b.read_value("bin").unwrap_err();
    assert!(matches!(error, VaultError::Unreadable));
    assert!(!format!("{error:?} {error}").contains("255"));
}

#[test]
fn marks_refuse_older_records_other_keys_at_one_revision_and_retired_keys() {
    let marks = Marks::in_memory();
    marks.accept("scope-key/x", 0, 3, Some("aa")).unwrap();
    marks.accept("scope-key/x", 0, 3, Some("aa")).unwrap();
    assert!(marks.accept("scope-key/x", 0, 2, Some("aa")).is_err());
    assert!(marks.accept("scope-key/x", 0, 3, Some("bb")).is_err());
    marks.accept("scope-key/x", 0, 4, Some("bb")).unwrap();
    // aa was the key before: put back with a newer revision, it is still refused.
    assert!(marks.accept("scope-key/x", 0, 9, Some("aa")).is_err());
    // A higher term outranks a higher revision.
    marks.accept("scope-key/x", 1, 1, Some("bb")).unwrap();
    assert!(marks.accept("scope-key/x", 0, 99, Some("bb")).is_err());
    // Names are kept apart.
    marks.accept("value/x", 0, 1, None).unwrap();
}

#[test]
fn marks_are_kept_in_the_state_directory_and_a_restarted_station_still_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MARKS_FILE);
    let registry_dir = tempfile::tempdir().unwrap();
    let registry = registry(registry_dir.path());
    let key = Arc::new(SealingKey::generate());
    registry
        .publish_sealing_key(&StationSealingKey::new("a", key.public_key().to_text()))
        .unwrap();
    let vault = || {
        Vault::new(registry.clone(), "a", key.clone())
            .with_marks(Arc::new(Marks::open(&path).unwrap()))
    };
    let a = vault();
    a.put_scope_key(&op(), &[Recipient::named("a")], NOW)
        .unwrap();
    a.write_value("v", &op(), "one", SealedExpect::Absent, NOW)
        .unwrap();
    let first = registry.sealed_value(&value_name("v")).unwrap().unwrap();
    a.write_value("v", &op(), "two", SealedExpect::Revision(1), NOW)
        .unwrap();
    drop(a);
    registry.remove_sealed(&value_name("v")).unwrap();
    registry
        .store_sealed(
            &value_name("v"),
            &first.copies,
            "x",
            SealedExpect::Absent,
            NOW,
        )
        .unwrap();
    // A new process reads the marks the first one kept.
    assert!(matches!(
        vault().read_value("v"),
        Err(VaultError::Rollback(_))
    ));
    std::fs::write(&path, "not marks").unwrap();
    let refused = Marks::open(&path).unwrap_err().to_string();
    assert!(
        refused.contains(MARKS_FILE) && !refused.contains("not marks"),
        "{refused}"
    );
}

// Phase 2: the scope a reader means, the move off per-Station seals, and requests answered by a
// Station for a gateway that holds no key.

use crate::directorates::q::sealing::{RequestStatus, SecretRequest, ask, status};

/// The review's sixth probe: a value another scope holds under the name was accepted.
#[test]
fn a_reader_that_names_a_scope_is_refused_a_value_another_scope_holds_and_it_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (_registry, a, b, c) = fleet(dir.path());
    let evil = Scope::operation("evil").unwrap();
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    c.put_scope_key(&evil, &named(&["a", "c"]), NOW).unwrap();
    // c holds only the evil scope's key: it cannot take the value down or write over it.
    assert!(matches!(
        c.remove_value_in("forge", &evil),
        Err(VaultError::WrongScope)
    ));
    assert!(matches!(
        c.write_value("forge", &evil, "attacker-chosen", SealedExpect::Any, NOW),
        Err(VaultError::WrongScope)
    ));
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
    // A reader that means the evil scope is not given the other scope's value, nor the reverse.
    assert!(matches!(
        a.read_value_in("forge", Some(&evil)),
        Err(VaultError::WrongScope)
    ));
    assert_eq!(
        a.read_value_in("forge", Some(&op()))
            .unwrap()
            .value
            .as_str(),
        PLANTED
    );
}

#[test]
fn values_sealed_to_each_station_move_under_the_global_scope_key_once() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, c) = fleet(dir.path());
    a.seal_to(
        "legacy",
        PLANTED,
        &named(&["a", "b"]),
        SealedExpect::Absent,
        NOW,
    )
    .unwrap();
    a.seal_to(
        "login/pool/acct",
        "kept",
        &named(&["a"]),
        SealedExpect::Absent,
        NOW,
    )
    .unwrap();
    c.seal_to("theirs", "x", &named(&["c"]), SealedExpect::Absent, NOW)
        .unwrap();
    assert_eq!(a.migrate_legacy(NOW).unwrap(), ["legacy"]);
    let global = Scope::global();
    for vault in [&a, &b] {
        let read = vault.read_value_in("legacy", Some(&global)).unwrap();
        assert_eq!(read.value.as_str(), PLANTED);
    }
    assert!(c.read_value("legacy").is_err(), "c was never named");
    assert!(registry.sealed_value("legacy").unwrap().is_none());
    // A login keeps its per-Station seal, and a value of another Station stays for it.
    assert_eq!(a.open("login/pool/acct").unwrap().value.as_str(), "kept");
    assert!(registry.sealed_value("theirs").unwrap().is_some());
    assert!(a.migrate_legacy(NOW).unwrap().is_empty());
    // c holds no wrap of the global key, so it cannot put its own value under it.
    assert!(c.migrate_legacy(NOW).unwrap().is_empty());
    assert_eq!(c.open("theirs").unwrap().value.as_str(), "x");
}

fn put(name: &str, scope: &str, value: &str, stations: &[&str]) -> SecretRequest {
    SecretRequest::Put {
        name: name.to_owned(),
        scope: Some(scope.to_owned()),
        value: value.to_owned(),
        expect: None,
        stations: stations
            .iter()
            .map(|station| (*station).to_owned())
            .collect(),
        pins: BTreeMap::new(),
    }
}

fn done(
    registry: &Arc<dyn RegistryBackend>,
    id: &str,
) -> crate::directorates::q::sealing::SecretOutcome {
    match status(registry.as_ref(), id).unwrap() {
        RequestStatus::Done(done) => done,
        other => panic!("{other:?}"),
    }
}

#[test]
fn one_put_through_a_request_is_read_by_two_stations_and_no_record_holds_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path());
    let (a, b, _c) = (
        station(&registry, "a"),
        station(&registry, "b"),
        station(&registry, "c"),
    );
    // The gateway: no vault, no key. It asks both Stations; whichever takes it does it.
    ask(
        registry.as_ref(),
        "req-1",
        &put("forge", "operation:alpha", PLANTED, &["a", "b"]),
        &named(&["a", "b"]),
        NOW,
    )
    .unwrap();
    assert_eq!(
        status(registry.as_ref(), "req-1").unwrap(),
        RequestStatus::Pending
    );
    assert_eq!(a.serve_requests(NOW).unwrap(), ["req-1"]);
    assert!(
        b.serve_requests(NOW).unwrap().is_empty(),
        "it was taken once"
    );
    let outcome = done(&registry, "req-1");
    assert_eq!(outcome.verb, "put");
    assert_eq!(outcome.revision, Some(1));
    assert_eq!(outcome.stations, ["a", "b"]);
    for vault in [&a, &b] {
        let read = vault.read_value_in("forge", Some(&op())).unwrap();
        assert_eq!(read.value.as_str(), PLANTED);
    }
    // The request is gone, and nothing the Registry holds carries the value.
    assert!(
        registry
            .sealed_value("secret-request/req-1")
            .unwrap()
            .is_none()
    );
    assert!(
        registry
            .sealed_value("secret-claim/req-1")
            .unwrap()
            .is_none()
    );
    let mut seen = String::new();
    for entry in walk(dir.path()) {
        seen.push_str(&String::from_utf8_lossy(&std::fs::read(entry).unwrap()));
    }
    seen.push_str(&format!("{outcome:?}"));
    assert!(!seen.contains(PLANTED), "{seen}");
    assert!(!seen.contains(&base64_text(PLANTED.as_bytes())), "{seen}");
}

#[test]
fn a_revoke_through_a_request_shuts_the_station_out_of_the_next_revision_and_says_what_remains() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry(dir.path());
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let send = |id: &str, request: &SecretRequest, to: &[&str]| {
        ask(registry.as_ref(), id, request, &named(to), NOW).unwrap();
        a.serve_requests(NOW).unwrap();
        status(registry.as_ref(), id).unwrap()
    };
    send(
        "r1",
        &put("forge", "operation:alpha", "before", &["a", "b"]),
        &["a"],
    );
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), "before");
    let revoked = send(
        "r2",
        &SecretRequest::Revoke {
            scope: "operation:alpha".to_owned(),
            station: "b".to_owned(),
        },
        &["a"],
    );
    let RequestStatus::Done(revoked) = revoked else {
        panic!("{revoked:?}");
    };
    assert_eq!(revoked.stations, ["a"]);
    let notes = revoked.notes.join(" ");
    assert!(notes.contains("keeps every value"), "{notes}");
    assert!(notes.contains("Registry credentials"), "{notes}");
    send("r3", &put("forge", "operation:alpha", PLANTED, &[]), &["a"]);
    assert_eq!(a.read_value("forge").unwrap().value.as_str(), PLANTED);
    assert!(matches!(b.read_value("forge"), Err(VaultError::NotNamed)));
}

#[test]
fn a_key_rotation_through_a_request_leaves_every_value_readable_by_the_stations_that_remain() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, b, _c) = fleet(dir.path());
    for name in ["one", "two"] {
        a.write_value(name, &op(), name, SealedExpect::Absent, NOW)
            .unwrap();
    }
    let before = a.scope_key_info(&op()).unwrap().unwrap();
    ask(
        registry.as_ref(),
        "rk",
        &SecretRequest::RotateKey {
            scope: "operation:alpha".to_owned(),
        },
        &named(&["b"]),
        NOW,
    )
    .unwrap();
    assert_eq!(b.serve_requests(NOW).unwrap(), ["rk"]);
    let outcome = done(&registry, "rk");
    assert_ne!(outcome.key_id.as_deref(), Some(before.key_id.as_str()));
    for vault in [&a, &b] {
        for name in ["one", "two"] {
            assert_eq!(vault.read_value(name).unwrap().value.as_str(), name);
        }
    }
}

#[test]
fn a_request_a_station_cannot_do_is_answered_with_why_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, a, _b, c) = fleet(dir.path());
    // c holds no wrap of the scope key.
    ask(
        registry.as_ref(),
        "no-wrap",
        &put("forge", "operation:alpha", PLANTED, &[]),
        &named(&["c"]),
        NOW,
    )
    .unwrap();
    assert_eq!(c.serve_requests(NOW).unwrap(), ["no-wrap"]);
    let RequestStatus::Failed(why) = status(registry.as_ref(), "no-wrap").unwrap() else {
        panic!("it should have failed");
    };
    assert!(why.contains("not sealed to this Station"), "{why}");
    assert!(!why.contains(PLANTED));
    assert!(a.read_value("forge").is_err(), "nothing was written");
    // A request for no Station, a bad ID and an unknown request.
    assert!(
        ask(
            registry.as_ref(),
            "x",
            &put("v", "global", "1", &[]),
            &[],
            NOW
        )
        .is_err()
    );
    assert!(
        ask(
            registry.as_ref(),
            "../x",
            &put("v", "global", "1", &[]),
            &named(&["a"]),
            NOW
        )
        .is_err()
    );
    assert!(matches!(
        status(registry.as_ref(), "never"),
        Err(VaultError::NotFound)
    ));
    let debug = format!("{:?}", put("v", "global", PLANTED, &[]));
    assert!(!debug.contains(PLANTED), "{debug}");
}

fn a_request_is_answered_and_a_value_moves_off_its_per_station_seal_on_every_backend(
    store: &dyn Store,
) {
    let (a, b, _c) = fleet_on(store);
    let registry: Arc<dyn RegistryBackend> = store.handle();
    ask(
        registry.as_ref(),
        "req-1",
        &SecretRequest::Put {
            name: "forge".to_owned(),
            scope: Some("operation:alpha".to_owned()),
            value: PLANTED.to_owned(),
            expect: None,
            stations: Vec::new(),
            pins: BTreeMap::new(),
        },
        &named(&["a"]),
        NOW,
    )
    .unwrap();
    assert_eq!(a.serve_requests(NOW).unwrap(), ["req-1"]);
    assert!(matches!(
        status(registry.as_ref(), "req-1").unwrap(),
        RequestStatus::Done(_)
    ));
    assert!(
        registry
            .sealed_value("secret-request/req-1")
            .unwrap()
            .is_none()
    );
    // Only a, which made the key, holds it; b is granted and then reads.
    assert!(matches!(b.read_value("forge"), Err(VaultError::NotNamed)));
    a.grant_station(&op(), &Recipient::named("b"), NOW).unwrap();
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
    a.seal_to(
        "legacy",
        "old",
        &named(&["a", "b"]),
        SealedExpect::Absent,
        NOW,
    )
    .unwrap();
    assert_eq!(a.migrate_legacy(NOW).unwrap(), ["legacy"]);
    assert_eq!(b.read_value("legacy").unwrap().value.as_str(), "old");
}
