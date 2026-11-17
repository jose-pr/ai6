//! Who a policy names: one account, one person, one group or every signed-in person.

use core::fmt;
use core::str::FromStr;

use crate::model::Person;

/// One account, person or group, written `account:<name>`, `person:<id>` or `group:<name>`, or
/// every signed-in person, written `any:signed-in`.
///
/// It is the text form policies store; [`matches`](Subject::matches) says whether a
/// directory's [`Person`] is the one it names. `Debug` shows the kind only, since the value
/// is an identity; `any:signed-in` names nobody, so it shows whole.
///
/// ```
/// use coqui_access::{Person, Subject};
///
/// let staff: Subject = "group:staff".parse()?;
/// let person = Person { groups: vec!["staff".into()], ..Person::default() };
/// assert!(staff.matches(&person));
/// assert_eq!(staff.to_string(), "group:staff");
///
/// let signed_in: Subject = "any:signed-in".parse()?;
/// assert!(signed_in.matches(&Person { id: "p-1".into(), ..Person::default() }));
/// assert!(!signed_in.matches(&Person::default()));
/// # Ok::<(), coqui_access::SubjectError>(())
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Subject {
    /// The person whose [`Person::name`] is this.
    Account(String),
    /// The person whose [`Person::id`] is this.
    Person(String),
    /// Every person whose [`Person::groups`] holds this.
    Group(String),
    /// Every person who carries an identity: a non-empty [`Person::id`]. An anonymous caller
    /// has none, so it never matches. It is the only reserved `any:` name.
    AnySignedIn,
}

impl Subject {
    /// The kind as written before the colon: `account`, `person`, `group` or `any`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Account(_) => "account",
            Self::Person(_) => "person",
            Self::Group(_) => "group",
            Self::AnySignedIn => "any",
        }
    }

    /// The name, id or group after the colon (`signed-in` for `any:signed-in`).
    pub fn value(&self) -> &str {
        match self {
            Self::Account(value) | Self::Person(value) | Self::Group(value) => value,
            Self::AnySignedIn => "signed-in",
        }
    }

    /// Whether `person` is the one this names, is in the group it names, or, for
    /// `any:signed-in`, has an id.
    pub fn matches(&self, person: &Person) -> bool {
        match self {
            Self::Account(name) => name == &person.name,
            Self::Person(id) => id == &person.id,
            Self::Group(group) => person.groups.iter().any(|member| member == group),
            Self::AnySignedIn => !person.id.is_empty(),
        }
    }
}

impl fmt::Debug for Subject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnySignedIn => formatter.write_str("Subject(any:signed-in)"),
            _ => write!(formatter, "Subject({}:…)", self.kind()),
        }
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.kind(), self.value())
    }
}

/// Text that is not `account:<name>`, `person:<id>` or `group:<name>` with a value that is
/// not empty and holds no control character, or `any:signed-in`. Any other `any:` name is
/// refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SubjectError;

impl fmt::Display for SubjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("a subject is account:<name>, person:<id>, group:<name> or any:signed-in")
    }
}

impl core::error::Error for SubjectError {}

impl FromStr for Subject {
    type Err = SubjectError;

    fn from_str(text: &str) -> Result<Self, SubjectError> {
        if text == "any:signed-in" {
            return Ok(Self::AnySignedIn);
        }
        let (kind, value) = text.split_once(':').ok_or(SubjectError)?;
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(SubjectError);
        }
        let value = value.to_owned();
        match kind {
            "account" => Ok(Self::Account(value)),
            "person" => Ok(Self::Person(value)),
            "group" => Ok(Self::Group(value)),
            _ => Err(SubjectError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person() -> Person {
        Person {
            id: "p-1".into(),
            name: "alice".into(),
            groups: vec!["staff".into()],
            ..Person::default()
        }
    }

    #[test]
    fn each_kind_reads_writes_and_matches_its_own_field() {
        for (text, matched) in [
            ("account:alice", true),
            ("person:p-1", true),
            ("group:staff", true),
            ("account:p-1", false),
            ("person:alice", false),
            ("group:alice", false),
        ] {
            let subject: Subject = text.parse().unwrap();
            assert_eq!(subject.to_string(), text);
            assert_eq!(subject.matches(&person()), matched, "{text}");
        }
        assert_eq!(
            "group:a:b".parse::<Subject>(),
            Ok(Subject::Group("a:b".into()))
        );
    }

    #[test]
    fn any_signed_in_reads_writes_and_matches_a_person_with_an_id() {
        let subject: Subject = "any:signed-in".parse().unwrap();
        assert_eq!(subject, Subject::AnySignedIn);
        assert_eq!(subject.to_string(), "any:signed-in");
        assert_eq!((subject.kind(), subject.value()), ("any", "signed-in"));
        assert_eq!(format!("{subject:?}"), "Subject(any:signed-in)");
        assert!(subject.matches(&person()));
        let anonymous = Person {
            groups: vec!["staff".into()],
            name: "alice".into(),
            ..Person::default()
        };
        assert!(!subject.matches(&anonymous), "an empty id is not signed in");
    }

    #[test]
    fn every_other_any_name_is_refused() {
        for text in [
            "any:",
            "any:other",
            "any:person",
            "any:signed-in:x",
            "any:Signed-in",
            "any:signed-in ",
            "ANY:signed-in",
            "any",
        ] {
            assert_eq!(text.parse::<Subject>(), Err(SubjectError), "{text:?}");
        }
        // The kind comes first, so a name that spells it is still a name.
        assert_eq!(
            "group:any:signed-in".parse::<Subject>(),
            Ok(Subject::Group("any:signed-in".into()))
        );
    }

    #[test]
    fn malformed_text_is_refused_and_debug_hides_the_value() {
        for text in ["", "alice", "user:alice", "group:", "group:a\nb", ":x"] {
            assert_eq!(text.parse::<Subject>(), Err(SubjectError), "{text:?}");
        }
        let debug = format!("{:?}", Subject::Account("alice".into()));
        assert!(!debug.contains("alice") && debug.contains("account"));
    }
}
