// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Laying one profile over another.

use super::{Expect, Profile};

pub(super) fn merge(base: &Profile, later: &Profile) -> Profile {
    let mut out = base.clone();
    for (key, expect) in later.entries() {
        match expect {
            Expect::Any => {
                out.remove(key);
            }
            other => {
                out.insert(key.clone(), other.clone());
            }
        }
    }
    out
}
