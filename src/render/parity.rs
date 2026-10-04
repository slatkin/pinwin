//! Display-free parity harness for the two grid painters (gsk-render-nodes
//! task 1.1): render a [`gsk::RenderNode`] and a cairo painter into two
//! [`cairo::ImageSurface`]s with no display and diff the pixels.
//!
//! `gsk::RenderNode::draw` renders a node to a plain cairo context without a
//! display, so the node emitter is testable exactly like the cairo painter.
//! The one API that cannot run headless is `gtk4::Snapshot::new()`: it
//! asserts GTK initialization, which needs a display. A `GtkSnapshot` is a
//! plain GObject though, so the tests build it with `glib::Object::new` —
//! the same type the production emitter writes into.

use gtk4::glib;
use gtk4::gsk;
use gtk4::prelude::SnapshotExt as _;

/// A `gtk4::Snapshot` for a display-free test (see the module docs).
pub(super) fn snapshot() -> gtk4::Snapshot {
    glib::Object::new()
}

/// An opaque ARGB32 surface of `width`×`height` logical pixels at device
/// scale `scale`, filled with `backdrop` first: both painters then blend
/// their fractional edges against the same colours, the way the real panel
/// draws over an opaque window.
pub(super) fn backed_surface(
    width: i32,
    height: i32,
    scale: f64,
    backdrop: [u8; 3],
) -> cairo::ImageSurface {
    let surface = cairo::ImageSurface::create(
        cairo::Format::ARgb32,
        (f64::from(width) * scale).ceil() as i32,
        (f64::from(height) * scale).ceil() as i32,
    )
    .expect("surface");
    surface.set_device_scale(scale, scale);
    let [r, g, b] = backdrop;
    {
        let cr = cairo::Context::new(&surface).expect("context");
        cr.set_source_rgb(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
        );
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();
    }
    surface
}

/// Draw `node` onto `surface` (in the surface's logical coordinates).
pub(super) fn draw_node(node: &gsk::RenderNode, surface: &cairo::ImageSurface) {
    let cr = cairo::Context::new(surface).expect("context");
    node.draw(&cr);
}

/// The per-pixel difference between two same-size surfaces.
#[derive(Clone, Copy, Debug)]
pub(super) struct Diff {
    /// Pixels whose channels are not all identical.
    pub differing: usize,
    /// The largest per-channel difference among differing pixels (0 when
    /// the surfaces are identical).
    pub max_delta: u8,
}

/// Compare `a` against `b` pixel by pixel. cairo's ARGB32 byte order is
/// B, G, R, A; all four channels count. The surfaces must have the same
/// dimensions.
pub(super) fn diff(a: &mut cairo::ImageSurface, b: &mut cairo::ImageSurface) -> Diff {
    a.flush();
    b.flush();
    let (width, height) = (a.width(), a.height());
    assert_eq!((width, height), (b.width(), b.height()), "same dimensions");
    let stride = a.stride() as usize;
    let a_data = a.data().expect("surface data");
    let b_data = b.data().expect("surface data");
    let mut result = Diff {
        differing: 0,
        max_delta: 0,
    };
    for y in 0..height as usize {
        let a_row = &a_data[y * stride..y * stride + usize::try_from(width).unwrap() * 4];
        let b_row = &b_data[y * stride..y * stride + usize::try_from(width).unwrap() * 4];
        for (a_px, b_px) in a_row
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b_row.as_chunks::<4>().0)
        {
            let delta = a_px
                .iter()
                .zip(b_px)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            if delta > 0 {
                result.differing += 1;
                result.max_delta = result.max_delta.max(delta);
            }
        }
    }
    result
}

/// Assert exact pixel parity — the scale-1 requirement.
pub(super) fn assert_exact(a: &mut cairo::ImageSurface, b: &mut cairo::ImageSurface, what: &str) {
    let diff = diff(a, b);
    assert_eq!(diff.differing, 0, "{what}: {diff:?}");
}

/// Assert parity within a stated per-channel tolerance — the scale-1.5
/// requirement: every differing pixel's largest channel delta is at most
/// `tolerance`.
pub(super) fn assert_within(
    a: &mut cairo::ImageSurface,
    b: &mut cairo::ImageSurface,
    tolerance: u8,
    what: &str,
) {
    let diff = diff(a, b);
    assert!(
        diff.max_delta <= tolerance,
        "{what}: {diff:?} exceeds tolerance {tolerance}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use gtk4::gdk;
    use gtk4::graphene;

    /// Task 1.1's unit test: a flat colour node drawn through
    /// `gsk::RenderNode::draw` matches the cairo painter pixel for pixel,
    /// with no display.
    #[test]
    fn a_flat_colour_node_matches_the_cairo_painter() {
        let colour = gdk::RGBA::new(0.8, 0.2, 0.1, 1.0);
        let bounds = graphene::Rect::new(2.0, 3.0, 10.0, 10.0);

        let mut cairo_side = backed_surface(20, 20, 1.0, [0, 0, 0]);
        {
            let cr = cairo::Context::new(&cairo_side).expect("context");
            cr.set_source_rgb(0.8, 0.2, 0.1);
            cr.rectangle(2.0, 3.0, 10.0, 10.0);
            let _ = cr.fill();
        }

        let snapshot = snapshot();
        snapshot.append_color(&colour, &bounds);
        let node = snapshot.to_node().expect("snapshot produced a node");
        let mut node_side = backed_surface(20, 20, 1.0, [0, 0, 0]);
        draw_node(&node, &node_side);

        assert_exact(&mut cairo_side, &mut node_side, "flat colour node");
    }
}
