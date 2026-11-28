use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::directorates::q::sealing::{
    Marks, Recipient, RequestStatus, Scope, SealingKey, SecretRequest, Vault, ask, status,
};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::{SealedExpect, StationSealingKey};
use crate::directorates::registry::store::FileRegistry;

const NOW: f64 = 1_790_000_000.0;
const PLANTED: &str = "planted-secret-0451";
const KEY_RECORD: &str = "scope-key/operation:alpha";

fn blank() -> SealedCopy {
    SealedCopy {
        key: String::new(),
        sealed: String::new(),
    }
}

/// A key record's copies without the signature a signer added.
fn unsigned_part(
    record: &crate::directorates::registry::sealed::SealedRecord,
) -> BTreeMap<String, SealedCopy> {
    record
        .copies
        .iter()
        .filter(|(label, _)| !label.starts_with("s:"))
        .map(|(label, copy)| (label.clone(), copy.clone()))
        .collect()
}

fn op() -> Scope {
    Scope::operation("alpha").unwrap()
}

fn purposes(all: &[Purpose]) -> BTreeSet<Purpose> {
    all.iter().copied().collect()
}

fn both() -> BTreeSet<Purpose> {
    purposes(&[Purpose::ScopeKeyRecords, Purpose::SecretRequests])
}

/// A root a person trusts and M's key under it.
struct Authority {
    root: SigningKey,
    m: Arc<SigningKey>,
}

impl Authority {
    fn new() -> Self {
        Self {
            root: SigningKey::generate(),
            m: Arc::new(SigningKey::generate()),
        }
    }

    fn trust(&self) -> Trust {
        Trust::strict(vec![self.root.public_key()])
    }

    fn certificate(&self, key: &SigningKey, serial: u64, window: (f64, f64)) -> Certificate {
        Certificate::issue(&self.root, "m", &key.public_key(), both(), serial, window)
    }

    /// M's signer at `term`, certified for a day around NOW.
    fn signer(&self, term: u64) -> Signer {
        let certificate = self.certificate(&self.m, 1, (NOW - 100.0, NOW + 86_400.0));
        Signer::new(self.m.clone(), certificate).with_term(move || Some(term))
    }
}

fn registry(dir: &std::path::Path) -> Arc<dyn RegistryBackend> {
    Arc::new(FileRegistry::open(dir))
}

fn station(
    registry: &Arc<dyn RegistryBackend>,
    name: &str,
    trust: Trust,
    signer: Option<Signer>,
) -> Vault {
    let key = Arc::new(SealingKey::generate());
    registry
        .publish_sealing_key(&StationSealingKey::new(name, key.public_key().to_text()))
        .unwrap();
    let vault = Vault::new(registry.clone(), name, key)
        .with_trust(trust)
        .with_marks(Arc::new(Marks::in_memory()))
        .with_clock(|| NOW);
    match signer {
        Some(signer) => vault.with_signer(signer),
        None => vault,
    }
}

fn named(stations: &[&str]) -> Vec<Recipient> {
    stations
        .iter()
        .map(|name| Recipient::named(*name))
        .collect()
}

/// `a` and `b` in the scope, both able to sign as M at term 1, and `c` outside it that cannot.
fn fleet(
    dir: &std::path::Path,
    authority: &Authority,
) -> (Arc<dyn RegistryBackend>, Vault, Vault, Vault) {
    let registry = registry(dir);
    let a = station(&registry, "a", authority.trust(), Some(authority.signer(1)));
    let b = station(&registry, "b", authority.trust(), Some(authority.signer(1)));
    let c = station(&registry, "c", authority.trust(), None);
    a.put_scope_key(&op(), &named(&["a", "b"]), NOW).unwrap();
    a.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    (registry, a, b, c)
}

#[test]
fn a_record_m_signed_is_read_by_every_station_and_carries_its_term_and_revision() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let (registry, a, b, _c) = fleet(dir.path(), &authority);
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
    let record = registry.sealed_value(KEY_RECORD).unwrap().unwrap();
    assert_eq!(record.revision, 1);
    assert_eq!(record.copies[SIGNATURE].key, "1");
    assert!(record.copies.contains_key(CERTIFICATE));
    // A grant and a rotation are records too, and each is signed in the revision it gets.
    a.grant_station(&op(), &Recipient::named("b"), NOW).unwrap();
    a.rotate_scope_key(&op(), NOW).unwrap();
    assert_eq!(b.read_value("forge").unwrap().value.as_str(), PLANTED);
    let record = registry.sealed_value(KEY_RECORD).unwrap().unwrap();
    assert!(record.revision > 1);
}

#[test]
fn an_unsigned_record_is_refused_by_a_station_that_pins_a_root() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let registry = registry(dir.path());
    let writer = station(&registry, "w", Trust::unsigned(), None);
    let reader = station(&registry, "r", authority.trust(), None);
    writer
        .put_scope_key(&op(), &named(&["w", "r"]), NOW)
        .unwrap();
    assert!(matches!(
        reader.scope_key_info(&op()),
        Err(VaultError::Unsigned(_))
    ));
    let none = station(&registry, "n", Trust::default(), None);
    assert!(matches!(
        none.scope_key_info(&op()),
        Err(VaultError::Unsigned(_))
    ));
    assert!(none.trust().weakening().unwrap().contains("pins no root"));
    assert!(
        Trust::unsigned()
            .weakening()
            .unwrap()
            .contains("accepted unsigned")
    );
    assert!(authority.trust().weakening().is_none());
}

/// A Station outside the scope forges a rotation, unsigned or signed by
/// a key of its own, and its records are refused.
#[test]
fn a_forged_rotation_is_refused_whether_unsigned_or_signed_by_a_key_no_root_certified() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let (registry, a, b, _c) = fleet(dir.path(), &authority);
    let held = registry.sealed_value(KEY_RECORD).unwrap().unwrap();
    // A record naming another key ID, with the real record's signature copied onto it.
    let mut forged = held.copies.clone();
    let real_id = forged
        .keys()
        .find(|label| label.starts_with("i:"))
        .unwrap()
        .clone();
    forged.remove(&real_id);
    forged.insert("i:00000000000000000000000000000000".to_owned(), blank());
    registry
        .store_sealed(KEY_RECORD, &forged, "c", SealedExpect::Revision(1), NOW)
        .unwrap();
    for victim in [&a, &b] {
        let refused = victim.read_value("forge").unwrap_err();
        assert!(
            matches!(&refused, VaultError::Untrusted(_, why) if why.contains("does not cover")),
            "{refused}"
        );
    }
    // The same record with no signature at all.
    forged.retain(|label, _| !label.starts_with("s:"));
    registry
        .store_sealed(KEY_RECORD, &forged, "c", SealedExpect::Revision(2), NOW)
        .unwrap();
    for victim in [&a, &b] {
        assert!(matches!(
            victim.read_value("forge"),
            Err(VaultError::Unsigned(_))
        ));
    }
    assert!(matches!(
        a.rotate_scope_key(&op(), NOW),
        Err(VaultError::Unsigned(_))
    ));
    // The same forgery signed by a root and a key the attacker made.
    let attacker = Authority::new();
    let certificate = attacker.certificate(&attacker.m, 1, (NOW - 100.0, NOW + 86_400.0));
    let signer = Signer::new(attacker.m.clone(), certificate).with_term(|| Some(1));
    let mut signed = forged.clone();
    let signature = signer.sign_record(op().as_str(), 1, 4, &forged);
    signed.extend(signature);
    registry
        .store_sealed(KEY_RECORD, &signed, "c", SealedExpect::Revision(3), NOW)
        .unwrap();
    let refused = b.read_value("forge").unwrap_err();
    assert!(
        matches!(&refused, VaultError::Untrusted(_, why) if why.contains("not signed by a root")),
        "{refused}"
    );
}

/// A revoked Station puts the record from before its revoke back. The
/// signed revision is the old one, so it fails even on a Station that remembers nothing.
#[test]
fn a_rolled_back_record_is_refused_even_by_a_station_with_no_marks() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let (registry, a, _b, _c) = fleet(dir.path(), &authority);
    let before = registry.sealed_value(KEY_RECORD).unwrap().unwrap();
    a.revoke_station(&op(), "b", NOW).unwrap();
    registry
        .store_sealed(KEY_RECORD, &before.copies, "b", SealedExpect::Any, NOW)
        .unwrap();
    // `a` has its marks; a fresh vault of the same Station has none.
    let fresh = Vault::new(registry.clone(), "a", a.key.clone())
        .with_trust(authority.trust())
        .with_clock(|| NOW);
    for vault in [&a, &fresh] {
        let refused = vault
            .write_value("forge", &op(), "after", SealedExpect::Any, NOW)
            .unwrap_err();
        assert!(
            matches!(refused, VaultError::Rollback(_) | VaultError::Untrusted(..)),
            "{refused}"
        );
    }
    let refused = fresh.read_value("forge").unwrap_err();
    assert!(
        matches!(&refused, VaultError::Untrusted(_, why) if why.contains("revision")),
        "{refused}"
    );
}

#[test]
fn an_expired_m_certificate_is_refused_and_so_is_one_not_yet_valid() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let registry = registry(dir.path());
    let clock = Arc::new(AtomicU64::new(NOW.to_bits()));
    let short = authority.certificate(&authority.m, 1, (NOW - 100.0, NOW + 10.0));
    let signer = Signer::new(authority.m.clone(), short).with_term(|| Some(1));
    let a = station(&registry, "a", authority.trust(), Some(signer));
    let ticking = clock.clone();
    let reader = station(&registry, "r", authority.trust(), None)
        .with_clock(move || f64::from_bits(ticking.load(Ordering::SeqCst)));
    a.put_scope_key(&op(), &named(&["a", "r"]), NOW).unwrap();
    assert!(reader.scope_key_info(&op()).unwrap().is_some());
    clock.store((NOW + 20.0).to_bits(), Ordering::SeqCst);
    let refused = reader.scope_key_info(&op()).unwrap_err();
    assert!(
        matches!(&refused, VaultError::Untrusted(_, why) if why.contains("expired")),
        "{refused}"
    );
    clock.store((NOW - 1_000.0).to_bits(), Ordering::SeqCst);
    let refused = reader.scope_key_info(&op()).unwrap_err();
    assert!(
        matches!(&refused, VaultError::Untrusted(_, why) if why.contains("not valid yet")),
        "{refused}"
    );
}

/// After an election the old Headquarters, still signing in its own term, is refused: by its own
/// Station before it writes, and by every other Station when its record is planted.
#[test]
fn a_former_headquarters_in_a_lower_term_is_refused_after_an_election() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let registry = registry(dir.path());
    // The same key and certificate, opened by whichever Station holds the lease.
    let old = station(
        &registry,
        "old",
        authority.trust(),
        Some(authority.signer(1)),
    );
    let new = station(
        &registry,
        "new",
        authority.trust(),
        Some(authority.signer(2)),
    );
    let reader = station(&registry, "r", authority.trust(), None);
    old.put_scope_key(&op(), &named(&["old", "new", "r"]), NOW)
        .unwrap();
    old.write_value("forge", &op(), PLANTED, SealedExpect::Absent, NOW)
        .unwrap();
    assert!(reader.read_value("forge").is_ok());
    // The election: the new Headquarters writes in term 2, and the verifiers accept it.
    new.rotate_scope_key(&op(), NOW).unwrap();
    assert!(reader.read_value("forge").is_ok());
    let held = registry.sealed_value(KEY_RECORD).unwrap().unwrap();
    assert_eq!(held.copies[SIGNATURE].key, "2");
    // The old one has not noticed. Its own marks stop it before it writes.
    let refused = old.revoke_station(&op(), "r", NOW).unwrap_err();
    assert!(matches!(refused, VaultError::Rollback(_)), "{refused}");
    // A record it signed in term 1 at the next revision is valid but below the mark.
    let mut planted = unsigned_part(&held);
    let next = held.revision + 1;
    let signature = authority
        .signer(1)
        .sign_record(op().as_str(), 1, next, &planted);
    planted.extend(signature);
    registry
        .store_sealed(
            KEY_RECORD,
            &planted,
            "old",
            SealedExpect::Revision(held.revision),
            NOW,
        )
        .unwrap();
    let refused = reader.read_value("forge").unwrap_err();
    assert!(matches!(refused, VaultError::Rollback(_)), "{refused}");
}

#[test]
fn a_replaced_m_key_is_refused_once_the_next_certificate_is_seen() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let (registry, a, b, _c) = fleet(dir.path(), &authority);
    let reader_marks = Arc::new(Marks::in_memory());
    let reader = Vault::new(registry.clone(), "b", b.key.clone())
        .with_trust(authority.trust())
        .with_marks(reader_marks)
        .with_clock(|| NOW);
    assert!(reader.read_value("forge").is_ok());
    // The root certifies M's next key at serial 2; M signs every record again with it.
    let next = Arc::new(SigningKey::generate());
    let certificate = authority.certificate(&next, 2, (NOW - 100.0, NOW + 86_400.0));
    let resigner = station(
        &registry,
        "m2",
        authority.trust(),
        Some(Signer::new(next, certificate).with_term(|| Some(1))),
    );
    assert_eq!(resigner.resign_scope_keys(NOW).unwrap(), 1);
    assert!(reader.read_value("forge").is_ok());
    // The old key, leaked, signs a record at the next revision: its serial is below the one seen.
    let held = registry.sealed_value(KEY_RECORD).unwrap().unwrap();
    let mut forged = unsigned_part(&held);
    forged.insert("i:11111111111111111111111111111111".to_owned(), blank());
    let signature = authority
        .signer(1)
        .sign_record(op().as_str(), 1, held.revision + 1, &forged);
    forged.extend(signature);
    registry
        .store_sealed(
            KEY_RECORD,
            &forged,
            "c",
            SealedExpect::Revision(held.revision),
            NOW,
        )
        .unwrap();
    let refused = reader.read_value("forge").unwrap_err();
    assert!(matches!(refused, VaultError::Rollback(_)), "{refused}");
    drop(a);
}

fn grant(scope: &str, station: &str) -> SecretRequest {
    SecretRequest::Grant {
        scope: scope.to_owned(),
        station: station.to_owned(),
        pin: None,
    }
}

fn failure(registry: &Arc<dyn RegistryBackend>, id: &str) -> String {
    match status(registry.as_ref(), id).unwrap() {
        RequestStatus::Failed(why) => why,
        other => panic!("{other:?}"),
    }
}

fn requester(authority: &Authority) -> Signer {
    Signer::new(
        authority.m.clone(),
        authority.certificate(&authority.m, 1, (NOW - 100.0, NOW + 86_400.0)),
    )
}

#[test]
fn a_request_the_requester_signed_is_served_and_one_unsigned_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let (registry, a, _b, c) = fleet(dir.path(), &authority);
    let signer = requester(&authority);
    ask(
        registry.as_ref(),
        "good",
        &grant("operation:alpha", "c"),
        &named(&["a"]),
        Some((&signer, 600.0)),
        NOW,
    )
    .unwrap();
    ask(
        registry.as_ref(),
        "bare",
        &grant("operation:alpha", "c"),
        &named(&["a"]),
        None,
        NOW,
    )
    .unwrap();
    let mut answered = a.serve_requests(NOW).unwrap();
    answered.sort();
    assert_eq!(answered, ["bare", "good"]);
    assert!(matches!(
        status(registry.as_ref(), "good").unwrap(),
        RequestStatus::Done(_)
    ));
    assert!(failure(&registry, "bare").contains("not signed"));
    // Only the signed one changed anything: c was granted once, by the signed request.
    assert!(c.read_value("forge").is_ok());
}

#[test]
fn a_request_signed_by_an_uncertified_key_or_expired_or_altered_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let (registry, a, _b, _c) = fleet(dir.path(), &authority);
    let outsider = Authority::new();
    ask(
        registry.as_ref(),
        "outsider",
        &grant("operation:alpha", "c"),
        &named(&["a"]),
        Some((&requester(&outsider), 600.0)),
        NOW,
    )
    .unwrap();
    ask(
        registry.as_ref(),
        "stale",
        &grant("operation:alpha", "c"),
        &named(&["a"]),
        Some((&requester(&authority), 10.0)),
        NOW - 100.0,
    )
    .unwrap();
    // A signed request whose content was swapped for another after it was signed.
    ask(
        registry.as_ref(),
        "swapped",
        &grant("operation:alpha", "b"),
        &named(&["a"]),
        Some((&requester(&authority), 600.0)),
        NOW,
    )
    .unwrap();
    let mut record = registry
        .sealed_value("secret-request/swapped")
        .unwrap()
        .unwrap();
    let swapped = serde_json::to_vec(&grant("operation:alpha", "c")).unwrap();
    record.copies.insert(
        "a".to_owned(),
        SealedCopy {
            key: a.key.public_key().to_text(),
            sealed: base64_text(&a.key.public_key().seal(&swapped).unwrap()),
        },
    );
    registry
        .store_sealed(
            "secret-request/swapped",
            &record.copies,
            "c",
            SealedExpect::Any,
            NOW,
        )
        .unwrap();
    a.serve_requests(NOW).unwrap();
    assert!(failure(&registry, "outsider").contains("not signed by a root"));
    assert!(failure(&registry, "stale").contains("expired"));
    assert!(failure(&registry, "swapped").contains("does not cover"));
    let held = a.scope_key_info(&op()).unwrap().unwrap();
    assert_eq!(
        held.stations,
        BTreeSet::from(["a".to_owned(), "b".to_owned()])
    );
}

#[test]
fn a_station_that_holds_no_signing_key_leaves_a_key_change_for_one_that_does() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let (registry, a, _b, c) = fleet(dir.path(), &authority);
    // c holds no wrap, but it is asked as well: it must neither answer nor claim.
    let signer = requester(&authority);
    ask(
        registry.as_ref(),
        "g",
        &grant("operation:alpha", "c"),
        &named(&["a", "c"]),
        Some((&signer, 600.0)),
        NOW,
    )
    .unwrap();
    assert!(c.serve_requests(NOW).unwrap().is_empty());
    assert_eq!(
        status(registry.as_ref(), "g").unwrap(),
        RequestStatus::Pending
    );
    assert_eq!(a.serve_requests(NOW).unwrap(), ["g"]);
    assert!(matches!(
        status(registry.as_ref(), "g").unwrap(),
        RequestStatus::Done(_)
    ));
}

#[test]
fn m_s_key_sealed_in_the_registry_opens_only_for_the_stations_it_is_sealed_to() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let registry = registry(dir.path());
    let a = station(&registry, "a", authority.trust(), None);
    let b = station(&registry, "b", authority.trust(), None);
    let outsider = station(&registry, "x", authority.trust(), None);
    let certificate = authority.certificate(&authority.m, 1, (NOW - 100.0, NOW + 86_400.0));
    a.seal_m_key(&authority.m, &certificate, &named(&["a", "b"]), NOW)
        .unwrap();
    assert!(outsider.open_m_signer(|| Some(1)).unwrap().is_none());
    let signer = b.open_m_signer(|| Some(7)).unwrap().unwrap();
    assert_eq!(signer.term().unwrap(), 7);
    let b = b.with_signer(signer);
    b.put_scope_key(&op(), &named(&["b"]), NOW).unwrap();
    let held = registry.sealed_value(KEY_RECORD).unwrap().unwrap();
    assert_eq!(held.copies[SIGNATURE].key, "7");
    // With no live Headquarters there is no term, and nothing is signed.
    let no_lease = a
        .open_m_signer(|| None)
        .unwrap()
        .map(|signer| a.with_signer(signer))
        .unwrap();
    assert!(matches!(
        no_lease.grant_station(&op(), &Recipient::named("a"), NOW),
        Err(VaultError::NotNamed | VaultError::NoTerm)
    ));
}

#[test]
fn trust_files_and_certificates_round_trip_and_a_changed_certificate_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Authority::new();
    let path = dir.path().join(TRUST_FILE);
    assert_eq!(TrustFile::open(&path).unwrap(), TrustFile::default());
    let file = TrustFile {
        roots: vec![authority.root.public_key().to_text()],
        unsigned: false,
        signer_file: None,
    };
    file.save(&path).unwrap();
    assert_eq!(TrustFile::open(&path).unwrap(), file);
    assert_eq!(file.trust().unwrap(), authority.trust());
    let bad = TrustFile {
        roots: vec!["ed25519:nothex".to_owned()],
        ..TrustFile::default()
    };
    assert!(bad.trust().is_err());
    let certificate = authority.certificate(&authority.m, 1, (NOW - 1.0, NOW + 1.0));
    let round = Certificate::from_json(&certificate.to_json()).unwrap();
    assert_eq!(round, certificate);
    let mut widened = certificate.clone();
    widened.not_after += 1e9;
    assert!(
        authority
            .trust()
            .certified(&widened, Purpose::ScopeKeyRecords, NOW)
            .is_err()
    );
    let narrow = Certificate::issue(
        &authority.root,
        "gateway",
        &authority.m.public_key(),
        purposes(&[Purpose::SecretRequests]),
        1,
        (NOW - 1.0, NOW + 1.0),
    );
    assert!(
        authority
            .trust()
            .certified(&narrow, Purpose::ScopeKeyRecords, NOW)
            .unwrap_err()
            .contains("does not allow")
    );
    // Widening what a certificate allows breaks the root's signature.
    let mut upgraded = narrow.clone();
    upgraded.purposes.insert(Purpose::ScopeKeyRecords);
    assert!(
        authority
            .trust()
            .certified(&upgraded, Purpose::ScopeKeyRecords, NOW)
            .unwrap_err()
            .contains("not signed by a root")
    );
}
