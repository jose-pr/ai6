//! Bash patterns (`*`, `?`, `[...]`, `\x`) and the backtracking match the string operators use.

use crate::error::{Error, Limit};
use alloc::vec::Vec;

#[derive(Debug)]
enum Tok {
    Lit(char),
    Any,
    Star,
    Class {
        negate: bool,
        items: Vec<(char, char)>,
    },
}

/// A compiled bash pattern.
#[derive(Debug)]
pub(crate) struct Glob(Vec<Tok>);

impl Glob {
    pub(crate) fn new(pattern: &str) -> Self {
        let chars: Vec<char> = pattern.chars().collect();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\\' if i + 1 < chars.len() => {
                    toks.push(Tok::Lit(chars[i + 1]));
                    i += 2;
                    continue;
                }
                '*' => toks.push(Tok::Star),
                '?' => toks.push(Tok::Any),
                '[' => {
                    let close = (i + 2..chars.len()).find(|k| chars[*k] == ']');
                    if let Some(close) = close {
                        let mut body = &chars[i + 1..close];
                        let negate = body.first() == Some(&'!');
                        if negate {
                            body = &body[1..];
                        }
                        let mut items = Vec::new();
                        let mut k = 0;
                        while k < body.len() {
                            if k + 2 < body.len() && body[k + 1] == '-' {
                                items.push((body[k], body[k + 2]));
                                k += 3;
                            } else {
                                items.push((body[k], body[k]));
                                k += 1;
                            }
                        }
                        toks.push(Tok::Class { negate, items });
                        i = close + 1;
                        continue;
                    }
                    toks.push(Tok::Lit('['));
                }
                c => toks.push(Tok::Lit(c)),
            }
            i += 1;
        }
        Self(toks)
    }

    /// The end of the match at `start` that a regular expression would prefer: greedy stars try
    /// the longest first, `lazy` ones the shortest. With `to_end` the match must reach the end.
    pub(crate) fn match_at(
        &self,
        s: &[char],
        start: usize,
        lazy: bool,
        to_end: bool,
        budget: &mut usize,
    ) -> Result<Option<usize>, Error> {
        self.step(0, s, start, lazy, to_end, budget)
    }

    fn step(
        &self,
        ti: usize,
        s: &[char],
        si: usize,
        lazy: bool,
        to_end: bool,
        budget: &mut usize,
    ) -> Result<Option<usize>, Error> {
        if *budget == 0 {
            return Err(Error::Limit(Limit::Iterations));
        }
        *budget -= 1;
        let Some(tok) = self.0.get(ti) else {
            return Ok((!to_end || si == s.len()).then_some(si));
        };
        match tok {
            Tok::Lit(c) => {
                if s.get(si) == Some(c) {
                    self.step(ti + 1, s, si + 1, lazy, to_end, budget)
                } else {
                    Ok(None)
                }
            }
            Tok::Any => {
                if si < s.len() {
                    self.step(ti + 1, s, si + 1, lazy, to_end, budget)
                } else {
                    Ok(None)
                }
            }
            Tok::Class { negate, items } => match s.get(si) {
                Some(c) if items.iter().any(|(lo, hi)| lo <= c && c <= hi) != *negate => {
                    self.step(ti + 1, s, si + 1, lazy, to_end, budget)
                }
                _ => Ok(None),
            },
            Tok::Star => {
                let order: Vec<usize> = if lazy {
                    (si..=s.len()).collect()
                } else {
                    (si..=s.len()).rev().collect()
                };
                for k in order {
                    if let Some(end) = self.step(ti + 1, s, k, lazy, to_end, budget)? {
                        return Ok(Some(end));
                    }
                }
                Ok(None)
            }
        }
    }
}
