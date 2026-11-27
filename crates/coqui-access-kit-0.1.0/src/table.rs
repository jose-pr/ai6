//! Typed tables a provider reads and writes through, whatever keeps them.
//!
//! A [`Table`] is the source of a provider's records: a [`MemoryTable`] built in code, a file
//! with a codec, a SQL database or an HTTP API are all tables, and the provider cannot tell
//! them apart. [`Reloading`] caches the latest [`Snapshot`] and refreshes it through
//! [`Table::changes_since`].

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use coqui_access::error;
use coqui_access::guatiao::library::ProviderError;

/// Why a table could not answer. Messages never contain record contents or secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceError {
    /// The table cannot be written: the default of [`Table::put`] and [`Table::remove`].
    ReadOnly,
    /// The records or the request are not acceptable: out of bounds, duplicated, malformed.
    Invalid(String),
    /// The table could not be reached or read.
    Unavailable(String),
}

impl SourceError {
    /// An [`Invalid`](SourceError::Invalid) error.
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::Invalid(reason.into())
    }

    /// An [`Unavailable`](SourceError::Unavailable) error.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable(reason.into())
    }

    /// What a provider answers when `table` could not be read: always
    /// `GUATIAO_ERR_INTERNAL`, "could not find out", whatever the reason. The message is
    /// `<table>: <reason>`.
    pub fn reading(&self, table: &str) -> ProviderError {
        error::internal(&format!("{table}: {self}"))
    }

    /// What a provider answers when `table` refused a change, or could not be opened from a
    /// configuration: [`ReadOnly`](SourceError::ReadOnly) is `GUATIAO_ERR_UNSUPPORTED`,
    /// [`Invalid`](SourceError::Invalid) `GUATIAO_ERR_BAD_VALUE`, anything else
    /// `GUATIAO_ERR_INTERNAL`. The message is `<table>: <reason>`.
    pub fn writing(&self, table: &str) -> ProviderError {
        let message = format!("{table}: {self}");
        match self {
            Self::ReadOnly => error::unsupported(&message),
            Self::Invalid(_) => error::bad_value(&message),
            Self::Unavailable(_) => error::internal(&message),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => formatter.write_str("the table is read only"),
            Self::Invalid(reason) | Self::Unavailable(reason) => formatter.write_str(reason),
        }
    }
}

impl core::error::Error for SourceError {}

/// Which state of a table a [`Snapshot`] is: opaque, compared only for equality, and chosen
/// by the table. `0` is what a table that never changes may use for ever.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(u64);

impl Revision {
    /// A revision with this number.
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A record a table can find again by name.
pub trait Keyed {
    /// What identifies this record in its table: a put with the same key replaces it.
    /// Borrowed when the record holds it as text, owned when it is written from a value.
    fn key(&self) -> Cow<'_, str>;
}

/// All the records of a table at one [`Revision`], in the table's order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Snapshot<T> {
    /// Which state of the table this is.
    pub revision: Revision,
    /// The records.
    pub records: Vec<T>,
}

impl<T> Snapshot<T> {
    /// A snapshot of `records` at `revision`.
    pub fn new(revision: Revision, records: Vec<T>) -> Self {
        Self { revision, records }
    }
}

/// What [`Table::update`] is given to decide a change: the records read, and the record to
/// put, if any.
pub type Update<'a, T> = dyn FnMut(&[T]) -> Result<Option<T>, SourceError> + 'a;

/// What [`Table::update_one`] is given to decide a change to one record: that record as the
/// table holds it (`None` when there is none), and the record to put, if any.
pub type UpdateOne<'a, T> = dyn FnMut(Option<&T>) -> Result<Option<T>, SourceError> + 'a;

/// Where a provider's records of type `T` come from, and optionally go to.
///
/// Implementations are free to keep records anywhere. They must be cheap to ask for changes:
/// [`Reloading`] calls [`changes_since`](Table::changes_since) on every use.
pub trait Table<T>: Send + Sync {
    /// Every record, with the revision they are at.
    ///
    /// # Errors
    ///
    /// Returns an error when the table cannot be read or its contents are unacceptable.
    fn load(&self) -> Result<Snapshot<T>, SourceError>;

    /// The records again when the table is no longer at revision `since`, else `None`.
    ///
    /// The default loads and compares revisions; a table that can tell cheaply overrides it.
    ///
    /// # Errors
    ///
    /// As [`load`](Table::load).
    fn changes_since(&self, since: Revision) -> Result<Option<Snapshot<T>>, SourceError> {
        let snapshot = self.load()?;
        Ok((snapshot.revision != since).then_some(snapshot))
    }

    /// Adds `record`, or replaces the one with the same [`Keyed::key`].
    ///
    /// # Errors
    ///
    /// [`SourceError::ReadOnly`] by default.
    fn put(&self, record: T) -> Result<(), SourceError> {
        let _ = record;
        Err(SourceError::ReadOnly)
    }

    /// Removes the record with this key; `true` when there was one.
    ///
    /// # Errors
    ///
    /// [`SourceError::ReadOnly`] by default.
    fn remove(&self, key: &str) -> Result<bool, SourceError> {
        let _ = key;
        Err(SourceError::ReadOnly)
    }

    /// Reads the records, lets `make` decide from them what to put (`None` puts nothing),
    /// and puts it.
    ///
    /// A table shared with another process overrides this to hold a lock from the read to
    /// the write, so `make` always sees the records its result replaces into. The default
    /// is a read and a put with nothing between them, which is as safe as the table's own
    /// `put`.
    ///
    /// # Errors
    ///
    /// When the table cannot be read, `make` refuses, or the put fails
    /// ([`SourceError::ReadOnly`] for a read-only table).
    fn update(&self, make: &mut Update<'_, T>) -> Result<(), SourceError> {
        let snapshot = self.load()?;
        match make(&snapshot.records)? {
            Some(record) => self.put(record),
            None => Ok(()),
        }
    }

    /// The record with this key, or `None`.
    ///
    /// The default loads every record and picks one, so a table that can find a record by
    /// its key (a database, an API, a record per file) overrides it: then a record that
    /// cannot be read is an error for its own key only, and a caller that serves one
    /// person at a time pays for one record.
    ///
    /// # Errors
    ///
    /// When the table, or this record, cannot be read.
    fn get(&self, key: &str) -> Result<Option<T>, SourceError>
    where
        T: Keyed,
    {
        let snapshot = self.load()?;
        Ok(snapshot
            .records
            .into_iter()
            .find(|record| record.key() == key))
    }

    /// Like [`update`](Table::update), for the one record with this key: `make` is given
    /// that record (or `None`) as the table holds it under its lock, and what it returns is
    /// put.
    ///
    /// The default is [`update`](Table::update) with the other records left out of
    /// `make`'s view; override it with [`get`](Table::get) to read one record.
    ///
    /// # Errors
    ///
    /// When the table or this record cannot be read, `make` refuses, or the put fails.
    fn update_one(&self, key: &str, make: &mut UpdateOne<'_, T>) -> Result<(), SourceError>
    where
        T: Keyed,
    {
        self.update(&mut |records| make(records.iter().find(|record| record.key() == key)))
    }

    /// Removes every record `condition` holds for, and answers how many went.
    ///
    /// The default loads the records and removes each through [`remove`](Table::remove), a
    /// write apiece, so a table that rewrites itself on every write overrides it to remove
    /// them all in one. Nothing matching writes nothing, so a read-only table answers 0.
    ///
    /// # Errors
    ///
    /// When the table cannot be read or a removal fails ([`SourceError::ReadOnly`] for a
    /// read-only table holding a matching record).
    fn remove_where(&self, condition: &mut dyn FnMut(&T) -> bool) -> Result<usize, SourceError>
    where
        T: Keyed,
    {
        let snapshot = self.load()?;
        let mut removed = 0;
        for record in snapshot.records.iter().filter(|record| condition(record)) {
            if self.remove(&record.key())? {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// A table kept in memory, built in code. It is writable and its revision counts the
/// changes made to it.
pub struct MemoryTable<T> {
    state: Mutex<(u64, Vec<T>)>,
}

impl<T> fmt::Debug for MemoryTable<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryTable")
            .finish_non_exhaustive()
    }
}

impl<T> Default for MemoryTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> MemoryTable<T> {
    /// An empty table.
    pub fn new() -> Self {
        Self {
            state: Mutex::new((0, Vec::new())),
        }
    }

    /// A table holding `records`, in this order. A key given twice keeps its last record.
    pub fn from_records(records: impl IntoIterator<Item = T>) -> Self
    where
        T: Keyed,
    {
        let table = Self::new();
        {
            let mut state = table.state.lock().unwrap_or_else(PoisonError::into_inner);
            for record in records {
                upsert(&mut state.1, record);
            }
        }
        table
    }
}

fn upsert<T: Keyed>(records: &mut Vec<T>, record: T) {
    match records.iter().position(|had| had.key() == record.key()) {
        Some(at) => records[at] = record,
        None => records.push(record),
    }
}

impl<T: Keyed + Clone + Send> Table<T> for MemoryTable<T> {
    fn load(&self) -> Result<Snapshot<T>, SourceError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Snapshot::new(Revision::new(state.0), state.1.clone()))
    }

    fn changes_since(&self, since: Revision) -> Result<Option<Snapshot<T>>, SourceError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(
            (state.0 != since.get())
                .then(|| Snapshot::new(Revision::new(state.0), state.1.clone())),
        )
    }

    fn put(&self, record: T) -> Result<(), SourceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        upsert(&mut state.1, record);
        state.0 += 1;
        Ok(())
    }

    fn update(&self, make: &mut Update<'_, T>) -> Result<(), SourceError> {
        // The table stays locked from the read to the write, so two updates never both
        // decide from the same records.
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(record) = make(&state.1)? {
            upsert(&mut state.1, record);
            state.0 += 1;
        }
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<T>, SourceError>
    where
        T: Keyed,
    {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(state.1.iter().find(|record| record.key() == key).cloned())
    }

    fn update_one(&self, key: &str, make: &mut UpdateOne<'_, T>) -> Result<(), SourceError>
    where
        T: Keyed,
    {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let current = state.1.iter().find(|record| record.key() == key);
        if let Some(record) = make(current)? {
            upsert(&mut state.1, record);
            state.0 += 1;
        }
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<bool, SourceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let before = state.1.len();
        state.1.retain(|record| record.key() != key);
        let removed = state.1.len() < before;
        if removed {
            state.0 += 1;
        }
        Ok(removed)
    }

    fn remove_where(&self, condition: &mut dyn FnMut(&T) -> bool) -> Result<usize, SourceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let before = state.1.len();
        state.1.retain(|record| !condition(record));
        let removed = before - state.1.len();
        if removed > 0 {
            state.0 += 1;
        }
        Ok(removed)
    }
}

type Validate<T> = Box<dyn Fn(&[T]) -> Result<(), SourceError> + Send + Sync>;

/// A table with its latest [`Snapshot`] cached, refreshed through
/// [`Table::changes_since`] on every use, so a write by anyone else is seen without
/// reopening.
///
/// Every snapshot is checked with the validation given to [`new`](Reloading::new); a table
/// that has become unreadable or invalid is an error, never a stale answer.
pub struct Reloading<T> {
    table: Arc<dyn Table<T>>,
    validate: Validate<T>,
    latest: Mutex<Snapshot<T>>,
}

impl<T> fmt::Debug for Reloading<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Reloading").finish_non_exhaustive()
    }
}

impl<T: Keyed + Clone> Reloading<T> {
    /// Loads `table` once, checking it with `validate`.
    ///
    /// # Errors
    ///
    /// Returns an error when the table cannot be loaded or `validate` refuses it.
    pub fn new(
        table: Arc<dyn Table<T>>,
        validate: impl Fn(&[T]) -> Result<(), SourceError> + Send + Sync + 'static,
    ) -> Result<Self, SourceError> {
        let snapshot = table.load()?;
        validate(&snapshot.records)?;
        Ok(Self {
            table,
            validate: Box::new(validate),
            latest: Mutex::new(snapshot),
        })
    }

    /// Runs `read` over the current records.
    ///
    /// # Errors
    ///
    /// Returns an error when the table changed and can no longer be read or is invalid.
    pub fn read<R>(&self, read: impl FnOnce(&[T]) -> R) -> Result<R, SourceError> {
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        self.refresh(&mut latest)?;
        Ok(read(&latest.records))
    }

    /// Makes one record from the current ones and puts it, or puts nothing when `make`
    /// refuses or the records with it would be invalid.
    ///
    /// # Errors
    ///
    /// Returns an error when the table cannot be read, `make` refuses, the result is
    /// invalid, or the table refuses the put ([`SourceError::ReadOnly`] for a read-only
    /// table).
    pub fn put_with<R>(
        &self,
        make: impl FnOnce(&[T]) -> Result<(T, R), SourceError>,
    ) -> Result<R, SourceError> {
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        let mut make = Some(make);
        let mut made = None;
        // `make` runs against the records the table read for this update (a file table reads
        // them under its lock), not against the last snapshot.
        self.table.update(&mut |records| {
            (self.validate)(records)?;
            let Some(make) = make.take() else {
                return Err(SourceError::invalid("the table asked twice"));
            };
            let (record, result) = make(records)?;
            let mut candidate = records.to_vec();
            upsert(&mut candidate, record.clone());
            (self.validate)(&candidate)?;
            made = Some(result);
            Ok(Some(record))
        })?;
        self.refresh(&mut latest)?;
        made.ok_or_else(|| SourceError::invalid("the table did not update"))
    }

    /// The record with `key`, read from the table itself and checked alone: neither the
    /// cache nor the other records are touched, so a record that cannot be read or fails
    /// `validate` is an error for its own key only.
    ///
    /// # Errors
    ///
    /// Returns an error when the table or the record cannot be read, or the record is
    /// invalid.
    pub fn get(&self, key: &str) -> Result<Option<T>, SourceError> {
        let found = self.table.get(key)?;
        if let Some(record) = &found {
            (self.validate)(std::slice::from_ref(record))?;
        }
        Ok(found)
    }

    /// Like [`put_with`](Reloading::put_with), for the one record with `key`: `make` is
    /// given it (or `None`) as the table reads it under its lock, and returns the record to
    /// put (`None` puts nothing) and a result. Only that record is read and checked, so
    /// one that cannot be read is an error for its own key only, and the cost does not
    /// grow with the table.
    ///
    /// # Errors
    ///
    /// Returns an error when the table or the record cannot be read, `make` refuses, the
    /// record or the result is invalid or has another key, or the table refuses the put.
    pub fn update_one<R>(
        &self,
        key: &str,
        make: impl FnOnce(Option<&T>) -> Result<(Option<T>, R), SourceError>,
    ) -> Result<R, SourceError> {
        let mut make = Some(make);
        let mut made = None;
        self.table.update_one(key, &mut |current| {
            if let Some(current) = current {
                (self.validate)(std::slice::from_ref(current))?;
            }
            let Some(make) = make.take() else {
                return Err(SourceError::invalid("the table asked twice"));
            };
            let (record, result) = make(current)?;
            if let Some(record) = &record {
                if record.key() != key {
                    return Err(SourceError::invalid("the record is not the one asked for"));
                }
                (self.validate)(std::slice::from_ref(record))?;
            }
            made = Some(result);
            Ok(record)
        })?;
        made.ok_or_else(|| SourceError::invalid("the table did not update"))
    }

    /// Removes the record with `key`; `true` when there was one.
    ///
    /// # Errors
    ///
    /// Returns an error when the table cannot be read or refuses the removal.
    pub fn remove(&self, key: &str) -> Result<bool, SourceError> {
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        self.refresh(&mut latest)?;
        let removed = self.table.remove(key)?;
        self.refresh(&mut latest)?;
        Ok(removed)
    }

    fn refresh(&self, latest: &mut Snapshot<T>) -> Result<(), SourceError> {
        if let Some(newer) = self.table.changes_since(latest.revision)? {
            (self.validate)(&newer.records)?;
            *latest = newer;
        }
        Ok(())
    }
}

/// Whether `value` may be a record's field: not empty, at most `max_bytes` long, and free of
/// control characters (which would let a field forge a line in a log or a file).
pub fn check_field(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

/// Now, in whole seconds since the Unix epoch, as records stamp `made_unix` and compare
/// `expires_unix`; `0` on a clock set before 1970.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or_default()
}

/// Checks that no key is used twice and that there are at most `max` records. A provider's
/// own validation calls this first.
///
/// # Errors
///
/// [`SourceError::Invalid`] naming which bound was broken, never the key.
pub fn check_keys<T: Keyed>(records: &[T], max: usize) -> Result<(), SourceError> {
    if records.len() > max {
        return Err(SourceError::invalid("the table holds too many records"));
    }
    let mut seen = BTreeSet::new();
    if records.iter().any(|record| !seen.insert(record.key())) {
        return Err(SourceError::invalid("a record is duplicated"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Name(String);

    impl Keyed for Name {
        fn key(&self) -> Cow<'_, str> {
            Cow::Borrowed(&self.0)
        }
    }

    fn name(text: &str) -> Name {
        Name(text.to_owned())
    }

    fn few(records: &[Name]) -> Result<(), SourceError> {
        check_keys(records, 2)
    }

    #[test]
    fn a_memory_table_counts_its_changes_and_reports_them() {
        let table = MemoryTable::from_records([name("a"), name("a"), name("b")]);
        let first = table.load().unwrap();
        assert_eq!(first.records, [name("a"), name("b")]);
        assert_eq!(table.changes_since(first.revision), Ok(None));
        table.put(name("c")).unwrap();
        let second = table.changes_since(first.revision).unwrap().unwrap();
        assert_eq!(second.records.len(), 3);
        assert_ne!(second.revision, first.revision);
        assert!(table.remove("c").unwrap());
        assert!(!table.remove("c").unwrap());
        // Removing nothing is not a change.
        let third = table.load().unwrap();
        assert!(!table.remove("missing").unwrap());
        assert_eq!(table.changes_since(third.revision), Ok(None));
    }

    #[test]
    fn a_table_that_implements_only_load_is_read_only() {
        struct Fixed;
        impl Table<Name> for Fixed {
            fn load(&self) -> Result<Snapshot<Name>, SourceError> {
                Ok(Snapshot::new(Revision::new(7), vec![name("a")]))
            }
        }
        assert_eq!(Fixed.changes_since(Revision::new(7)), Ok(None));
        assert!(Fixed.changes_since(Revision::new(1)).unwrap().is_some());
        assert_eq!(Fixed.put(name("b")), Err(SourceError::ReadOnly));
        assert_eq!(Fixed.remove("a"), Err(SourceError::ReadOnly));
        let reloading = Reloading::new(Arc::new(Fixed), few).unwrap();
        assert_eq!(reloading.read(<[Name]>::len).unwrap(), 1);
        assert_eq!(
            reloading.put_with(|_| Ok((name("b"), ()))),
            Err(SourceError::ReadOnly)
        );
    }

    #[test]
    fn reloading_sees_writes_made_through_another_handle() {
        let table = Arc::new(MemoryTable::<Name>::new());
        let one = Reloading::new(table.clone(), few).unwrap();
        let other = Reloading::new(table, few).unwrap();
        assert_eq!(one.read(<[Name]>::len).unwrap(), 0);
        other.put_with(|_| Ok((name("a"), ()))).unwrap();
        assert_eq!(one.read(<[Name]>::len).unwrap(), 1);
        assert!(one.remove("a").unwrap());
        assert_eq!(other.read(<[Name]>::len).unwrap(), 0);
    }

    #[test]
    fn a_refused_put_leaves_the_table_alone() {
        let table = Arc::new(MemoryTable::<Name>::new());
        let reloading = Reloading::new(table.clone(), few).unwrap();
        for text in ["a", "b"] {
            reloading.put_with(|_| Ok((name(text), ()))).unwrap();
        }
        // Over the bound, and refused by the maker.
        assert!(reloading.put_with(|_| Ok((name("c"), ()))).is_err());
        assert!(
            reloading
                .put_with(|_| Err::<(Name, ()), _>(SourceError::invalid("no")))
                .is_err()
        );
        assert_eq!(table.load().unwrap().records.len(), 2);
        // Replacing is not growing.
        reloading.put_with(|_| Ok((name("a"), ()))).unwrap();
    }

    #[test]
    fn a_table_that_goes_bad_is_an_error_not_a_stale_answer() {
        struct Flaky(AtomicUsize);
        impl Table<Name> for Flaky {
            fn load(&self) -> Result<Snapshot<Name>, SourceError> {
                match self.0.fetch_add(1, Ordering::SeqCst) {
                    0 => Ok(Snapshot::new(Revision::new(1), vec![name("a")])),
                    1 => Err(SourceError::unavailable("gone")),
                    _ => Ok(Snapshot::new(Revision::new(3), vec![name("a"), name("a")])),
                }
            }
        }
        let reloading = Reloading::new(Arc::new(Flaky(AtomicUsize::new(0))), few).unwrap();
        assert!(matches!(
            reloading.read(|_| ()),
            Err(SourceError::Unavailable(_))
        ));
        assert!(matches!(
            reloading.read(|_| ()),
            Err(SourceError::Invalid(_))
        ));
    }

    #[test]
    fn a_table_error_becomes_the_status_a_host_acts_on() {
        use coqui_access::guatiao::Status;

        let read_only = SourceError::ReadOnly;
        let invalid = SourceError::invalid("a record is duplicated");
        let gone = SourceError::unavailable("file cannot be read");
        for error in [&read_only, &invalid, &gone] {
            let read = error.reading("account table");
            assert_eq!(read.status, Status::GUATIAO_ERR_INTERNAL);
            assert_eq!(&*read.message, format!("account table: {error}"));
        }
        assert_eq!(
            read_only.writing("t").status,
            Status::GUATIAO_ERR_UNSUPPORTED
        );
        assert_eq!(invalid.writing("t").status, Status::GUATIAO_ERR_BAD_VALUE);
        assert_eq!(gone.writing("t").status, Status::GUATIAO_ERR_INTERNAL);
        assert_eq!(&*invalid.writing("t").message, "t: a record is duplicated");
    }

    #[test]
    fn a_field_is_bounded_non_empty_and_has_no_control_characters() {
        assert!(check_field("alice", 5));
        assert!(!check_field("alice", 4));
        assert!(!check_field("", 5));
        assert!(!check_field("a\nb", 5));
        assert!(!check_field("a\u{7f}", 5));
        assert!(check_field("é", 2));
        assert!(unix_now() > 1_700_000_000);
    }

    #[test]
    fn a_table_invalid_at_the_start_is_not_opened() {
        let table = Arc::new(MemoryTable::from_records([name("a"), name("b"), name("c")]));
        assert!(Reloading::new(table, few).is_err());
    }

    #[test]
    fn one_record_is_read_and_updated_by_its_key() {
        let table = MemoryTable::from_records([name("a"), name("b")]);
        assert_eq!(table.get("b").unwrap(), Some(name("b")));
        assert_eq!(table.get("z").unwrap(), None);
        let mut seen = Vec::new();
        table
            .update_one("a", &mut |current| {
                seen.push(current.cloned());
                Ok(Some(name("a")))
            })
            .unwrap();
        assert_eq!(seen, [Some(name("a"))]);
        assert_eq!(table.load().unwrap().revision, Revision::new(1));
    }

    #[test]
    fn the_defaults_find_one_record_through_load_and_update() {
        struct Plain(MemoryTable<Name>);
        impl Table<Name> for Plain {
            fn load(&self) -> Result<Snapshot<Name>, SourceError> {
                self.0.load()
            }
            fn put(&self, record: Name) -> Result<(), SourceError> {
                self.0.put(record)
            }
        }
        let table = Plain(MemoryTable::from_records([name("a"), name("b")]));
        assert_eq!(table.get("a").unwrap(), Some(name("a")));
        let mut seen = None;
        table
            .update_one("b", &mut |current| {
                seen = Some(current.cloned());
                Ok(Some(name("b")))
            })
            .unwrap();
        assert_eq!(seen, Some(Some(name("b"))));
    }

    #[test]
    fn remove_where_takes_every_match_in_one_change_or_one_remove_apiece() {
        let table = MemoryTable::from_records([name("a1"), name("b"), name("a2")]);
        let before = table.load().unwrap().revision;
        let removed = table
            .remove_where(&mut |record| record.0.starts_with('a'))
            .unwrap();
        assert_eq!(removed, 2);
        let after = table.load().unwrap();
        assert_eq!(after.records, [name("b")]);
        assert_eq!(after.revision.get(), before.get() + 1, "one change");
        assert_eq!(table.remove_where(&mut |_| false).unwrap(), 0);
        assert_eq!(table.load().unwrap().revision, after.revision);

        // The default removes through `remove`; with nothing matching it writes nothing.
        struct Removing(MemoryTable<Name>, AtomicUsize);
        impl Table<Name> for Removing {
            fn load(&self) -> Result<Snapshot<Name>, SourceError> {
                self.0.load()
            }
            fn remove(&self, key: &str) -> Result<bool, SourceError> {
                self.1.fetch_add(1, Ordering::SeqCst);
                self.0.remove(key)
            }
        }
        let table = Removing(
            MemoryTable::from_records([name("a1"), name("b"), name("a2")]),
            AtomicUsize::new(0),
        );
        assert_eq!(
            table.remove_where(&mut |record| record.0 == "c").unwrap(),
            0
        );
        assert_eq!(table.1.load(Ordering::SeqCst), 0);
        let removed = table
            .remove_where(&mut |record| record.0.starts_with('a'))
            .unwrap();
        assert_eq!((removed, table.1.load(Ordering::SeqCst)), (2, 2));
        assert_eq!(table.load().unwrap().records, [name("b")]);
    }

    #[test]
    fn reloading_checks_only_the_one_record_it_reads() {
        let table = Arc::new(MemoryTable::from_records([name("a"), name("b")]));
        let reloading = Reloading::new(table.clone(), |records| {
            if records.iter().any(|record| record.0 == "bad") {
                Err(SourceError::invalid("a record is invalid"))
            } else {
                Ok(())
            }
        })
        .unwrap();
        table.put(name("bad")).unwrap();
        assert!(reloading.read(|_| ()).is_err());
        assert_eq!(reloading.get("a").unwrap(), Some(name("a")));
        assert!(reloading.get("bad").is_err());
        let put = reloading
            .update_one("b", |current| {
                assert_eq!(current, Some(&name("b")));
                Ok((Some(name("b")), 7))
            })
            .unwrap();
        assert_eq!(put, 7);
        assert!(
            reloading
                .update_one("b", |_| Ok((Some(name("other")), ())))
                .is_err()
        );
    }
}
