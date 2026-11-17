//! An account's primary group. Active Directory leaves it out of `memberOf`, and names it by
//! the RID in `primaryGroupID`: the group's `objectSid` is the account's domain SID with that
//! RID in place of its own.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use coqui_access_kit::CacheOptions;

/// A security identifier: `S-1-<authority>-<sub-authority>...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sid {
    authority: u64,
    subs: Vec<u32>,
}

impl Sid {
    /// Decodes the binary form (revision 1, a count, a 48-bit big-endian authority, then
    /// little-endian 32-bit sub-authorities). Anything else, or trailing bytes, is `None`.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        let (&revision, rest) = bytes.split_first()?;
        let (&count, rest) = rest.split_first()?;
        let (authority, rest) = rest.split_at_checked(6)?;
        let count = usize::from(count);
        if revision != 1 || count == 0 || count > 15 || rest.len() != count * 4 {
            return None;
        }
        let authority = authority_value(authority);
        let subs = rest
            .as_chunks::<4>()
            .0
            .iter()
            .map(|part| u32::from_le_bytes(*part))
            .collect();
        Some(Self { authority, subs })
    }

    /// The SID of the group with `rid` in the same domain: this one without its last
    /// sub-authority (its own RID), then `rid`. `None` for a SID with no domain part.
    pub(crate) fn sibling(&self, rid: u32) -> Option<Self> {
        if self.subs.len() < 2 {
            return None;
        }
        let mut subs = self.subs.clone();
        *subs.last_mut()? = rid;
        Some(Self {
            authority: self.authority,
            subs,
        })
    }

    /// `S-1-5-21-...`.
    pub(crate) fn text(&self) -> String {
        let mut text = format!("S-1-{}", self.authority);
        for sub in &self.subs {
            text.push('-');
            text.push_str(&sub.to_string());
        }
        text
    }

    /// The binary form as a filter value, every byte `\xx`.
    pub(crate) fn filter_value(&self) -> String {
        let mut bytes = vec![1, self.subs.len() as u8];
        bytes.extend_from_slice(&self.authority.to_be_bytes()[2..]);
        for sub in &self.subs {
            bytes.extend_from_slice(&sub.to_le_bytes());
        }
        bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
    }
}

fn authority_value(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte))
}

/// A group found by SID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) dn: String,
    pub(crate) name: String,
}

/// Group SIDs already resolved (each names its domain), kept as the lookup cache is: found
/// for `ttl`, not found for `negative_ttl`, at most `max_entries`.
#[derive(Debug)]
pub(crate) struct GroupCache {
    options: CacheOptions,
    kept: Mutex<HashMap<String, (Instant, Option<Group>)>>,
}

impl GroupCache {
    pub(crate) fn new(options: CacheOptions) -> Self {
        Self {
            options,
            kept: Mutex::new(HashMap::new()),
        }
    }

    /// `Some(answer)` when `sid` was resolved recently, the answer being `None` for "no such
    /// group".
    pub(crate) fn get(&self, sid: &str) -> Option<Option<Group>> {
        let kept = self.kept.lock().unwrap_or_else(|lost| lost.into_inner());
        let (at, group) = kept.get(sid)?;
        let ttl = if group.is_some() {
            self.options.ttl
        } else {
            self.options.negative_ttl
        };
        (at.elapsed() < ttl).then(|| group.clone())
    }

    pub(crate) fn put(&self, sid: &str, group: Option<Group>) {
        let limit = usize::try_from(self.options.max_entries).unwrap_or(usize::MAX);
        if limit == 0 {
            return;
        }
        let mut kept = self.kept.lock().unwrap_or_else(|lost| lost.into_inner());
        if kept.len() >= limit && !kept.contains_key(sid) {
            let (ttl, negative) = (self.options.ttl, self.options.negative_ttl);
            kept.retain(|_, (at, group)| {
                at.elapsed() < if group.is_some() { ttl } else { negative }
            });
            if kept.len() >= limit {
                kept.clear();
            }
        }
        kept.insert(sid.to_owned(), (Instant::now(), group));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn binary(authority: u8, subs: &[u32]) -> Vec<u8> {
        let mut bytes = vec![1, subs.len() as u8, 0, 0, 0, 0, 0, authority];
        for sub in subs {
            bytes.extend_from_slice(&sub.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_binary_sid_reads_as_text_and_a_group_in_its_domain_has_its_rid() {
        let user = Sid::parse(&binary(5, &[21, 1, 2, 3, 1104])).unwrap();
        assert_eq!(user.text(), "S-1-5-21-1-2-3-1104");
        let group = user.sibling(513).unwrap();
        assert_eq!(group.text(), "S-1-5-21-1-2-3-513");
        assert_eq!(
            group.filter_value(),
            "\\01\\05\\00\\00\\00\\00\\00\\05\\15\\00\\00\\00\\01\\00\\00\\00\\02\\00\\00\\00\\03\\00\\00\\00\\01\\02\\00\\00"
        );
    }

    #[test]
    fn a_malformed_sid_is_nothing() {
        let good = binary(5, &[21, 1, 2, 3, 1104]);
        assert!(Sid::parse(&good).is_some());
        assert!(Sid::parse(&good[..good.len() - 1]).is_none());
        let mut longer = good.clone();
        longer.push(0);
        assert!(Sid::parse(&longer).is_none());
        let mut revision = good.clone();
        revision[0] = 2;
        assert!(Sid::parse(&revision).is_none());
        let mut many = good.clone();
        many[1] = 16;
        assert!(Sid::parse(&many).is_none());
        assert!(Sid::parse(&[]).is_none());
        assert!(Sid::parse(&[1]).is_none());
        // A SID with no domain part has no sibling.
        assert!(Sid::parse(&binary(1, &[0])).unwrap().sibling(513).is_none());
    }

    #[test]
    fn the_cache_keeps_found_and_missing_groups_for_their_own_times() {
        let group = Group {
            dn: "CN=Domain Users,DC=example".to_owned(),
            name: "Domain Users".to_owned(),
        };
        let cache = GroupCache::new(CacheOptions::default());
        assert!(cache.get("S-1-5-21-1-513").is_none());
        cache.put("S-1-5-21-1-513", Some(group.clone()));
        cache.put("S-1-5-21-1-514", None);
        assert_eq!(cache.get("S-1-5-21-1-513"), Some(Some(group)));
        assert_eq!(cache.get("S-1-5-21-1-514"), Some(None));
        let off = GroupCache::new(CacheOptions::off());
        off.put("S-1-5-21-1-513", None);
        assert!(off.get("S-1-5-21-1-513").is_none());
        let one = GroupCache::new(CacheOptions::default().max_entries(1));
        one.put("a", None);
        one.put("b", None);
        assert!(one.get("a").is_none());
        assert_eq!(one.get("b"), Some(None));
    }
}
