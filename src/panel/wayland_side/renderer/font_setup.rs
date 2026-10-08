//! The panel renderer's font setup (`replace-gtk-with-wayland`
//! D3): the one font load the panel thread performs, on the thread that
//! owns the text pass. It resolves the Ghostty `font-family` and
//! `font-size` through the font module — falling back to `monospace 11`,
//! which is normal operation and not an error — and measures the cell the
//! panel thread's sizing needs ([`CellSize`], the measurement the thread
//! performs before it binds its surfaces, so the surfaces' width and the
//! grid derivation have it from the first configure).
//!
//! A font that cannot be resolved at all — fontconfig unusable, the
//! resolved file unreadable or unparsable, or metrics no cell comes out
//! of — is a [`FontSetupError`], never a panic. The thread maps it onto
//! `PinwinError::Internal` at the start handshake.
//!
//! GTK-free (`replace-gtk-with-wayland` D10): the tests run without a
//! display.

use crate::fontconfig::{FontConfig, load_font_config};
use crate::layout::CellSize;
use crate::render::cell_metrics::{CellMetrics, MetricsError, measure};
use crate::render::font::{FamilyFaces, FontBook, FontError};

/// A font setup failed. Returned as an error — never a panic — and the
/// `monospace` fallback is not one of these: it is normal operation.
#[derive(Debug)]
pub enum FontSetupError {
    /// The font lookup failed (fontconfig unavailable, or the resolved
    /// family's file could not be read or parsed).
    Font(FontError),
    /// The cell metrics could not be measured from the resolved face.
    Metrics(MetricsError),
}

impl std::fmt::Display for FontSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Font(error) => write!(f, "the panel font did not resolve: {error}"),
            Self::Metrics(error) => write!(f, "the panel cell did not measure: {error}"),
        }
    }
}

impl std::error::Error for FontSetupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Font(error) => Some(error),
            Self::Metrics(error) => Some(error),
        }
    }
}

/// The panel thread's font: the font book the shaper resolves fallbacks
/// through, the family's faces, the measured cell metrics and the font
/// size in points the text pass shapes at. Built once per panel thread
/// and consumed by the [`super::Renderer`] constructor.
#[derive(Debug)]
pub struct FontSetup {
    book: FontBook,
    faces: FamilyFaces,
    cell_metrics: CellMetrics,
    size_pt: f64,
}

impl FontSetup {
    /// Resolve the font from the Ghostty config (the `font-family` and
    /// `font-size` [`load_font_config`] reads) and measure the cell. The
    /// entry the panel thread calls at its start.
    ///
    /// # Errors
    /// [`FontSetupError::Font`] when fontconfig cannot resolve a face at
    /// all, [`FontSetupError::Metrics`] when no cell comes out of the
    /// resolved face.
    pub fn load() -> Result<Self, FontSetupError> {
        Self::resolve(&load_font_config())
    }

    /// Resolve the font from a caller-supplied config and measure the
    /// cell. The configurable form of [`FontSetup::load`], which the tests
    /// drive at the test family.
    ///
    /// # Errors
    /// [`FontSetupError::Font`] when fontconfig cannot resolve a face at
    /// all, [`FontSetupError::Metrics`] when no cell comes out of the
    /// resolved face.
    pub fn resolve(config: &FontConfig) -> Result<Self, FontSetupError> {
        let book = FontBook::new().map_err(FontSetupError::Font)?;
        Self::resolve_over(book, config)
    }

    /// Resolve over a caller-built [`FontBook`] — the seam the tests
    /// inject a broken book through. Part of [`FontSetup::resolve`], kept
    /// a step apart so the book's construction stays the caller's.
    ///
    /// # Errors
    /// As [`FontSetup::resolve`].
    pub(crate) fn resolve_over(
        book: FontBook,
        config: &FontConfig,
    ) -> Result<Self, FontSetupError> {
        // The family resolution: a family fontconfig really
        // matches stays as asked, everything else — including a config
        // that names no family — falls back to `monospace` as normal
        // operation. Only a lookup that fails outright is an error.
        let faces = book.family_faces(config).map_err(FontSetupError::Font)?;
        let cell_metrics =
            measure(faces.regular(), config.size).map_err(FontSetupError::Metrics)?;
        Ok(Self {
            book,
            faces,
            cell_metrics,
            size_pt: config.size,
        })
    }

    /// The measured cell, the `CellSize` the panel thread's sizing needs
    /// (D3): the surfaces' width and the grid derivation both come from
    /// it. `None` only when the measured pitch is zero — which the whole
    /// positive pitch [`measure`] produces never is — so a `None` here is
    /// as unreachable as a zero cell.
    #[must_use]
    pub fn cell(&self) -> Option<CellSize> {
        CellSize::new(self.cell_metrics.cell_w(), self.cell_metrics.cell_h())
    }

    /// The parts the [`super::Renderer`] constructor consumes, moved out:
    /// the book, the family's faces, the cell metrics and the font size
    /// in points.
    pub fn into_parts(self) -> (FontBook, FamilyFaces, CellMetrics, f64) {
        let Self {
            book,
            faces,
            cell_metrics,
            size_pt,
        } = self;
        (book, faces, cell_metrics, size_pt)
    }
}
