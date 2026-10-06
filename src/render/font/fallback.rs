//! The per-code-point fallback lookup: fontconfig's ranked candidates for a
//! charset pattern holding just the code point, walked until a candidate
//! really covers it.

use ::fontconfig::{CharSet, Fontconfig, Pattern, UnicodeCoverage};

use super::error::FontError;
use super::face::{Face, load_face};

/// One uncached fallback lookup: fontconfig's ranked list for a charset
/// pattern holding just the code point, walked until a candidate really
/// covers it. Candidates that cannot be read or parsed are skipped — a
/// broken font file elsewhere on the system must not fail the panel.
pub(crate) fn lookup(fc: &Fontconfig, codepoint: char) -> Result<Option<Face>, FontError> {
    let mut charset = CharSet::new(fc)?;
    charset.add_char(codepoint)?;
    let mut pattern = Pattern::new(fc)?;
    pattern.add_charset(charset)?;
    let ranked = pattern.sort_fonts(UnicodeCoverage::Trim)?;
    for candidate in ranked.iter() {
        let (Ok(file), Ok(index)) = (candidate.filename(), candidate.face_index()) else {
            continue;
        };
        match load_face(file, index) {
            Ok(face) if face.covers(codepoint) => return Ok(Some(face)),
            // Unreadable, unparseable or non-covering candidates fall
            // through to the next one; anything else (fontconfig itself)
            // is an error.
            Ok(_) | Err(FontError::Read { .. } | FontError::NotAFont { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}
