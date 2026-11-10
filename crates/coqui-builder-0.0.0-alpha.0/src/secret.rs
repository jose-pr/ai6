// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Secret`]: a setting whose value is wiped from memory when it is dropped.
//!
//! `#[schema(sensitive)]` marks a place so that a report, a non-default list and every datum
//! read from it show `Type(***)`; it moves no memory. A `Secret<T>` is the setting that also
//! wipes: the value is zeroized when the `Secret` drops, it is a leaf marked sensitive in its
//! own description, and a datum read from it is sensitive too.
//!
//! What is wiped, with the `zeroize` feature on:
//!
//! - the value a `Secret` holds, when it drops (or is replaced);
//! - the text of a sensitive [`Datum`], when it drops;
//! - the text a `Source` hands over for a sensitive place, once
//!   [`Partial::fill_from`](crate::Partial::fill_from) has parsed it;
//! - the values an `Env` holds, when it drops; and the texts a `Partial` read through serde
//!   held, wherever they drop.
//!
//! What is not: a plain `String` under `#[schema(sensitive)]` (hold a `Secret<String>`), the
//! text a caller passes to [`Partial::set`](crate::Partial::set), a copy the caller takes with
//! [`Datum::text`](crate::Datum::text) or [`Secret::expose`], the process environment itself,
//! the buffers a file format's own parser keeps, the copies of a value's text that
//! `non_defaults`, `differences`, a `Profile` and its `Expect` texts make while they compare
//! it, and a default kept as text in a description. A value that reallocates while it grows
//! (a `String` pushed to) leaves the old buffer behind: build a `Secret` from a value that
//! will not grow. Without the feature there is no `Secret`, and a sensitive datum is only
//! masked.

use core::fmt;
use core::marker::PhantomData;

use alloc::string::String;
use zeroize::Zeroize;

use crate::datum::Datum;
use crate::describe::{Codec, Describe, Description, Leaf, LeafError};
use crate::fill::LeafSlot;
use crate::inspect::{Entries, Inspect};
use crate::path::Segment;

/// A value that is zeroized when it drops, and never printed.
///
/// `Debug` is `Secret(***)` and there is no `Display`. `PartialEq` compares the values and
/// is not constant-time. As a setting a `Secret<T>` is a leaf of `T`'s text, marked
/// sensitive in the description.
pub struct Secret<T: Zeroize> {
    value: T,
}

impl<T: Zeroize> Secret<T> {
    /// Holds `value`.
    pub fn new(value: T) -> Secret<T> {
        Secret { value }
    }

    /// The value. A copy made from it is not wiped by this crate.
    pub fn expose(&self) -> &T {
        &self.value
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl<T: Zeroize> From<T> for Secret<T> {
    fn from(value: T) -> Secret<T> {
        Secret::new(value)
    }
}

impl<T: Zeroize + Clone> Clone for Secret<T> {
    fn clone(&self) -> Secret<T> {
        Secret::new(self.value.clone())
    }
}

impl<T: Zeroize + PartialEq> PartialEq for Secret<T> {
    fn eq(&self, other: &Secret<T>) -> bool {
        self.value == other.value
    }
}

impl<T: Zeroize + Eq> Eq for Secret<T> {}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// The codec of a [`Secret`]: the text of its value, read and written by the value's own
/// [`Leaf`] impl, with every datum it yields marked sensitive.
pub struct SecretCodec<T>(PhantomData<fn() -> T>);

impl<T> fmt::Debug for SecretCodec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretCodec")
    }
}

impl<T: Leaf + Zeroize> Codec<Secret<T>> for SecretCodec<T> {
    const TYPE: &'static str = T::TYPE;
    const ORDERED: bool = T::ORDERED;
    const SENSITIVE: bool = true;

    fn to_text(value: &Secret<T>) -> String {
        value.expose().to_text()
    }
    fn from_text(text: &str) -> Result<Secret<T>, LeafError> {
        T::from_text(text).map(Secret::new)
    }
    fn same(a: &Secret<T>, b: &Secret<T>) -> bool {
        T::same(a.expose(), b.expose())
    }
    fn order(a: &Secret<T>, b: &Secret<T>) -> Option<core::cmp::Ordering> {
        T::order(a.expose(), b.expose())
    }
    fn choices() -> Option<&'static [&'static str]> {
        T::choices()
    }
    fn labels() -> &'static [(&'static str, &'static str)] {
        T::labels()
    }
}

impl<T: Leaf + Zeroize> Describe for Secret<T> {
    type Fill = LeafSlot<Secret<T>, SecretCodec<T>>;
    const LEAF: bool = true;

    fn description() -> Description {
        Description::leaf_of::<Secret<T>, SecretCodec<T>>()
    }
}

impl<T: Leaf + Zeroize> Inspect for Secret<T> {
    fn at(&self, at: &[Segment]) -> Option<Datum> {
        at.is_empty()
            .then(|| Datum::new(T::TYPE, self.value.to_text()).sensitive())
    }

    fn entries(&self, _at: &[Segment]) -> Option<Entries> {
        None
    }
}
