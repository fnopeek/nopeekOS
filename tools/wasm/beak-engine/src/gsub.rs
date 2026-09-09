//! GSUB ligature substitution — the step between "which characters" and
//! "which glyphs".
//!
//! **Why it exists.** An icon font does not map its symbol to a codepoint; it
//! maps the WORD to one glyph, as a ligature. `<i class="fos-icon">home</i>`
//! carries four characters that are each an empty glyph, and the picture is
//! the ligature of all four. Without this, that element measured 1 px where a
//! browser gives it 24.
//!
//! **Why fontdue does not do it.** `FontSettings::load_substitutions` only
//! makes the ligature GLYPHS rasterisable by index — fontdue has no shaper and
//! says so ("singular characters do not have enough context to be
//! substituted"). The substitution itself is ours.
//!
//! Only ligatures (GSUB lookup type 4), and only from the features that are ON
//! by default: `liga`, `clig`, `rlig`. `dlig` (discretionary) and `hlig`
//! (historical) are opt-in through `font-variant-ligatures`, and applying them
//! unasked would change body text nobody asked to change. Contextual lookups
//! (types 5–8) are not applied — an icon font does not use them, and half a
//! shaper is worse than none.

use alloc::vec::Vec;

/// The ligature substitutions of ONE face, keyed by the first component glyph.
///
/// Sorted by that first glyph so a lookup is a binary search: the common case
/// is a run whose glyphs are not the start of any ligature, and that case has
/// to cost almost nothing.
#[derive(Default)]
pub struct Ligatures {
    by_first: Vec<(u16, Vec<Lig>)>,
}

struct Lig {
    /// The components AFTER the first one.
    rest: Vec<u16>,
    glyph: u16,
}

impl Ligatures {
    pub fn is_empty(&self) -> bool {
        self.by_first.is_empty()
    }

    /// The longest ligature that starts at `glyphs[at]`, as
    /// `(ligature glyph, how many glyphs it consumes)`.
    ///
    /// Longest-first: a font with both `ffi` and `ff` must take `ffi` where it
    /// applies, or `ffi` never fires at all.
    pub fn apply(&self, glyphs: &[u16], at: usize) -> Option<(u16, usize)> {
        let first = *glyphs.get(at)?;
        let k = self.by_first.binary_search_by_key(&first, |(g, _)| *g).ok()?;
        let mut best: Option<(u16, usize)> = None;
        for lig in &self.by_first[k].1 {
            let n = lig.rest.len();
            if glyphs.len() < at + 1 + n {
                continue;
            }
            if glyphs[at + 1..at + 1 + n] == lig.rest[..] {
                let take = n + 1;
                if best.is_none_or(|(_, b)| take > b) {
                    best = Some((lig.glyph, take));
                }
            }
        }
        best
    }

    /// Read the ligature substitutions out of a font's GSUB table. A font with
    /// none — every subsetted face we ship — yields an empty table, and an
    /// empty table is what makes the fast path in `measure` legal.
    pub fn read(bytes: &[u8], index: u32) -> Ligatures {
        let mut out = Ligatures::default();
        let Ok(face) = ttf_parser::Face::parse(bytes, index) else { return out };
        let Some(gsub) = face.tables().gsub else { return out };

        // Which lookups belong to a default-on ligature feature.
        let mut wanted: Vec<u16> = Vec::new();
        for feature in gsub.features {
            let tag = feature.tag.to_bytes();
            if matches!(&tag, b"liga" | b"clig" | b"rlig") {
                for idx in feature.lookup_indices {
                    if !wanted.contains(&idx) {
                        wanted.push(idx);
                    }
                }
            }
        }
        // A font whose feature list names none of the three still gets its
        // ligature lookups read. Icon fonts are routinely built with a bare
        // GSUB and no feature record at all, and refusing them here would be
        // a standards-correct answer to the wrong question.
        let all = wanted.is_empty();

        let mut pairs: Vec<(u16, Lig)> = Vec::new();
        for (n, lookup) in gsub.lookups.into_iter().enumerate() {
            if !all && !wanted.contains(&(n as u16)) {
                continue;
            }
            for table in lookup.subtables.into_iter::<ttf_parser::gsub::SubstitutionSubtable>() {
                let ttf_parser::gsub::SubstitutionSubtable::Ligature(ls) = table else { continue };
                // The coverage index of the FIRST component picks the set.
                for (set_index, set) in ls.ligature_sets.into_iter().enumerate() {
                    let Some(first) = coverage_glyph(&ls.coverage, set_index as u16) else { continue };
                    for lig in set {
                        let rest: Vec<u16> = lig.components.into_iter().map(|g| g.0).collect();
                        if rest.is_empty() {
                            continue; // a one-component "ligature" substitutes nothing
                        }
                        pairs.push((first, Lig { rest, glyph: lig.glyph.0 }));
                    }
                }
            }
        }

        pairs.sort_by_key(|(g, _)| *g);
        for (g, lig) in pairs {
            match out.by_first.last_mut() {
                Some((last, v)) if *last == g => v.push(lig),
                _ => out.by_first.push((g, alloc::vec![lig])),
            }
        }
        out
    }
}

/// The glyph at coverage index `i` — the inverse of `Coverage::get`, which the
/// table does not offer because a ligature set is addressed BY that index.
fn coverage_glyph(cov: &ttf_parser::opentype_layout::Coverage, i: u16) -> Option<u16> {
    use ttf_parser::opentype_layout::Coverage;
    match cov {
        Coverage::Format1 { glyphs } => glyphs.get(i).map(|g| g.0),
        // In a range record `value` IS the coverage index of `start`
        // (OpenType: startCoverageIndex).
        Coverage::Format2 { records } => {
            for r in *records {
                let n = r.end.0.checked_sub(r.start.0)?;
                if i >= r.value && i <= r.value + n {
                    return Some(r.start.0 + (i - r.value));
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[(u16, &[u16], u16)]) -> Ligatures {
        let mut pairs: Vec<(u16, Lig)> = entries
            .iter()
            .map(|(f, rest, g)| (*f, Lig { rest: rest.to_vec(), glyph: *g }))
            .collect();
        pairs.sort_by_key(|(g, _)| *g);
        let mut out = Ligatures::default();
        for (g, lig) in pairs {
            match out.by_first.last_mut() {
                Some((last, v)) if *last == g => v.push(lig),
                _ => out.by_first.push((g, alloc::vec![lig])),
            }
        }
        out
    }

    /// `ffi` and `ff` both start at `f`; the LONGER one has to win, or `ffi`
    /// can never fire.
    #[test]
    fn the_longest_ligature_wins() {
        let t = table(&[(1, &[1], 100), (1, &[1, 2], 101)]);
        assert_eq!(t.apply(&[1, 1, 2], 0), Some((101, 3)));
        assert_eq!(t.apply(&[1, 1, 3], 0), Some((100, 2)));
        assert_eq!(t.apply(&[1, 3], 0), None);
        assert_eq!(t.apply(&[5, 1, 1], 0), None);
        // …and it applies at any offset, not just the start.
        assert_eq!(t.apply(&[5, 1, 1], 1), Some((100, 2)));
    }

    #[test]
    fn an_empty_table_answers_nothing() {
        let t = Ligatures::default();
        assert!(t.is_empty());
        assert_eq!(t.apply(&[1, 2, 3], 0), None);
    }

    /// A run that ends mid-ligature keeps its glyphs: `f` at the very end of a
    /// string is an `f`, not half an `ff`.
    #[test]
    fn a_truncated_sequence_does_not_match() {
        let t = table(&[(1, &[1, 2], 101)]);
        assert_eq!(t.apply(&[1, 1], 0), None);
    }
}
