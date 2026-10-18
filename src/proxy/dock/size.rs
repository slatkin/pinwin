//! The first dock's size decision (replace-terminal-with-app-host D4, D9):
//! whether the panel's settings resolve to a geometry now, or a percent size
//! has to wait for the output it is a percent of.
//!
//! An absolute-pixel size needs no output, so a pixel panel docks at once.
//! A percent size needs the output size, and the proxy never invents one: the
//! window waits until an output reports a size ([`super::super::output`]).
//! Once that size is known, both forms resolve exactly as they do for a live
//! apply ([`crate::geometry::Size::resolve`]), so the first map and every
//! later apply agree. The decision is pure and display-free (`port-to-rust`
//! D10), so its tests run without a compositor.

use crate::config::PanelSettings;
use crate::geometry::{Geometry, OutputSize, Size};

/// What the panel window's first dock does about its size (D4, D9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::proxy) enum DockDecision {
    /// The size resolved against `output`, when one was needed and known.
    Ready {
        geometry: Geometry,
        output: Option<OutputSize>,
    },
    /// A percent size needs its output, which is not known yet.
    Wait,
}

/// The first dock's decision for `settings` and the output size the panel's
/// output reports now, if any (D9): pixel sizes resolve without an output,
/// percent sizes only with one.
pub(in crate::proxy) fn decide(
    settings: &PanelSettings,
    output: Option<OutputSize>,
) -> DockDecision {
    let edge = settings.edge();
    let across = match settings.size() {
        Size::Pixels(px) => px.get(),
        Size::Percent(_) => {
            let Some(output) = output else {
                return DockDecision::Wait;
            };
            settings.size().resolve(edge, output)
        }
    };
    DockDecision::Ready {
        geometry: Geometry::new(edge, across, settings.gutters(), settings.reserve()),
        output,
    }
}

#[cfg(test)]
mod tests {
    use super::{DockDecision, decide};
    use crate::config::PanelSettings;
    use crate::geometry::{Edge, Geometry, OutputSize, Size};

    /// An output of `width` by `height` logical pixels.
    fn output(width: u32, height: u32) -> OutputSize {
        OutputSize::new(width, height).expect("a test output has two axes")
    }

    /// The geometry a ready decision carries.
    fn geometry(decision: DockDecision) -> Geometry {
        match decision {
            DockDecision::Ready { geometry, .. } => geometry,
            DockDecision::Wait => panic!("the test expects a resolved geometry"),
        }
    }

    /// An absolute-pixel size needs no output: the panel docks at once, so a
    /// pixel panel does not wait for an output to report a size (D9).
    #[test]
    fn a_pixel_size_docks_without_an_output() {
        let mut settings = PanelSettings::built_in();
        settings.size = Size::pixels(640).expect("a positive pixel size");
        let decision = decide(&settings, None);
        assert_eq!(geometry(decision).across(), 640);
        assert_eq!(
            decision,
            DockDecision::Ready {
                geometry: Geometry::new(Edge::Left, 640, settings.gutters(), settings.reserve(),),
                output: None,
            }
        );
    }

    /// A percent size without an output waits instead of resolving against
    /// an invented one (D9): the window is held until a size arrives.
    #[test]
    fn a_percent_size_waits_for_its_output() {
        assert_eq!(decide(&PanelSettings::built_in(), None), DockDecision::Wait);
    }

    /// The same percent resolves to the same width the live apply uses once
    /// the output size is known (spec "Percent of the output"): 25% of a
    /// 2560 pixel output is 640, and of a 1920 pixel output is 480.
    #[test]
    fn a_percent_size_resolves_against_the_output() {
        let settings = PanelSettings::built_in();
        let wide = decide(&settings, Some(output(2560, 1440)));
        assert_eq!(geometry(wide).across(), 640);
        assert_eq!(
            wide,
            DockDecision::Ready {
                geometry: Geometry::new(Edge::Left, 640, settings.gutters(), settings.reserve(),),
                output: Some(output(2560, 1440)),
            }
        );

        let narrow = decide(&settings, Some(output(1920, 1080)));
        assert_eq!(geometry(narrow).across(), 480);
    }
}
