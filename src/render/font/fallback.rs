//! The per-code-point fallback lookup: fontconfig's ranked candidates for a
//! charset pattern holding just the code point. Coverage is decided from
//! fontconfig's own `charset` data, so only a candidate that claims the code
//! point is read and parsed; a code point no candidate covers costs one
//! fontconfig sort (milliseconds) and no file reads at all.

use std::ptr;

use ::fontconfig::{CharSet, Fontconfig, Pattern, UnicodeCoverage};
use fontconfig_sys as sys;

use super::error::FontError;
use super::face::{Face, load_face};

/// How many ranked candidates the walk examines. fontconfig ranks the
/// candidates that cover the pattern's charset first, so a covering face
/// sits at the front; a code point none of the first `MAX_CANDIDATES`
/// covers is treated as not found. The cap bounds the walk: a not-found
/// code point costs one sort and at most `MAX_CANDIDATES` charset checks
/// (measured on the development machine, 3071 installed font patterns:
/// about 5 ms in total, most of it the sort), never the per-candidate file
/// reads the walk did before it consulted the charsets.
const MAX_CANDIDATES: usize = 256;

/// One uncached fallback lookup: the first candidate in fontconfig's ranking
/// that fontconfig itself claims covers the code point, confirmed with the
/// swash charmap on the loaded bytes. Candidates that cannot be read or
/// parsed are skipped — a broken font file elsewhere on the system must not
/// fail the panel — and an exhausted walk is a not-found answer for the
/// caller to cache.
pub(crate) fn lookup(fc: &Fontconfig, codepoint: char) -> Result<Option<Face>, FontError> {
    let mut charset = CharSet::new(fc)?;
    charset.add_char(codepoint)?;
    let mut pattern = Pattern::new(fc)?;
    pattern.add_charset(charset)?;
    let ranked = pattern.sort_fonts(UnicodeCoverage::Trim)?;
    for mut candidate in ranked.iter().take(MAX_CANDIDATES) {
        if !charset_covers(&mut candidate, codepoint) {
            continue;
        }
        let (Ok(file), Ok(index)) = (candidate.filename(), candidate.face_index()) else {
            continue;
        };
        // Only candidates fontconfig itself claims cover the code point are
        // read; the swash charmap has the last word on the loaded bytes.
        if let Ok(face) = load_face(file, index)
            && face.covers(codepoint)
        {
            return Ok(Some(face));
        }
        // Unreadable, unparseable, badly indexed or (despite the charset)
        // non-covering candidates fall through to the next one. The walk's
        // outcome — found or exhausted — is what the caller caches, so no
        // code point repeats even a failed walk.
    }
    Ok(None)
}

/// Whether the candidate pattern's own `charset` property contains the code
/// point. fontconfig computes the charset once per font when it builds its
/// cache, so this decides coverage without touching the font file.
///
/// The safe `fontconfig` wrapper exposes no charset accessor, so this calls
/// the underlying `yeslogic-fontconfig-sys` externs — the same library the
/// wrapper links. pinwin builds the wrapper in its default, non-`dlopen`
/// configuration, where the `-sys` externs bind to fontconfig directly; a
/// future `dlopen` build would have to route these through the wrapper's
/// own dispatch instead.
fn charset_covers(candidate: &mut Pattern<'_>, codepoint: char) -> bool {
    let mut charset: *mut sys::FcCharSet = ptr::null_mut();
    // SAFETY: `as_mut_ptr` hands out the live `FcPattern` the wrapper owns,
    // `FcPatternGetCharSet` only reads it, and the out pointer is a valid
    // `*mut FcCharSet` slot for the call.
    let status = unsafe {
        sys::FcPatternGetCharSet(
            candidate.as_mut_ptr(),
            sys::constants::FC_CHARSET.as_ptr(),
            0,
            &raw mut charset,
        )
    };
    if status != sys::FcResultMatch || charset.is_null() {
        // A pattern without a charset cannot prove coverage; skip it.
        return false;
    }
    // SAFETY: `charset` is the fontconfig-owned charset the previous call
    // returned, and `FcCharSetHasChar` only reads it.
    unsafe { sys::FcCharSetHasChar(charset, u32::from(codepoint)) != 0 }
}
