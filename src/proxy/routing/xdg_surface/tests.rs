//! The popup routing through a waiting first dock (D9): display-free, so the
//! tests run without a compositor (`port-to-rust` D10).

use std::rc::Rc;

use wl_proxy::baseline::Baseline;
use wl_proxy::object::ObjectUtils;
use wl_proxy::protocols::wayland::wl_surface::WlSurface;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wl_proxy::protocols::xdg_shell::xdg_popup::XdgPopup;
use wl_proxy::protocols::xdg_shell::xdg_positioner::{XdgPositioner, XdgPositionerHandler};
use wl_proxy::protocols::xdg_shell::xdg_surface::{XdgSurface, XdgSurfaceHandler};
use wl_proxy::protocols::xdg_shell::xdg_toplevel::XdgToplevel;
use wl_proxy::protocols::xdg_shell::xdg_wm_base::XdgWmBase;
use wl_proxy::state::State;

use super::ClientXdgSurface;
use crate::config::PanelSettings;
use crate::daemon::panel::AppId;
use crate::geometry::OutputSize;
use crate::proxy::popup::{ClientXdgPopup, ClientXdgPositioner};
use crate::proxy::routing::Routing;

/// A display-free state: the test mints proxy objects without a
/// compositor (`port-to-rust` D10).
fn state() -> Rc<State> {
    State::builder(Baseline::V6)
        .without_server()
        .build()
        .expect("a client-side state needs no compositor")
}

/// An output of `width` by `height` logical pixels.
fn output(width: u32, height: u32) -> OutputSize {
    OutputSize::new(width, height).expect("a test output has two axes")
}

/// The wm base the test's surfaces are created through.
fn wm_base(state: &Rc<State>) -> Rc<XdgWmBase> {
    state.create_object::<XdgWmBase>(1)
}

/// A held-back surface, installed on `xdg_surface` as the proxy's registry
/// does, so a popup of it finds the handler.
fn install_held_back(
    state: &Rc<State>,
    routing: &Rc<Routing>,
    surface: &Rc<WlSurface>,
    xdg_surface: &Rc<XdgSurface>,
) {
    xdg_surface.set_handler(ClientXdgSurface::held_back(
        Rc::clone(routing),
        Rc::clone(surface),
        Rc::clone(xdg_surface),
        wm_base(state),
        AppId::default(),
    ));
}

/// The panel window the test docks, with its window geometry set so a
/// popup of it takes the layer-popup branch.
fn panel_window(state: &Rc<State>, routing: &Rc<Routing>) -> (Rc<WlSurface>, Rc<XdgSurface>) {
    let surface = state.create_object::<WlSurface>(6);
    let xdg_surface = state.create_object::<XdgSurface>(1);
    install_held_back(state, routing, &surface, &xdg_surface);
    let toplevel = state.create_object::<XdgToplevel>(1);
    prepare_panel_window(&xdg_surface, &toplevel);
    (surface, xdg_surface)
}

/// Resumes the waiting dock through the installed panel handler, past its
/// deadline.
fn resume(xdg_surface: &Rc<XdgSurface>, routing: &Rc<Routing>) {
    let mut handler = xdg_surface
        .try_get_handler_mut::<ClientXdgSurface>()
        .expect("the held-back handler is installed");
    handler.resume(routing, true);
}

/// Destroys `positioner` the way an app does right after `get_popup`.
fn destroy_positioner(positioner: &Rc<XdgPositioner>) {
    let mut handler = positioner
        .try_get_handler_mut::<ClientXdgPositioner>()
        .expect("the positioner handler is installed");
    XdgPositionerHandler::handle_destroy(&mut *handler, positioner);
}

/// Takes the panel's first toplevel while its dock waits, with the window
/// geometry a popup of it offsets by (D5, D9).
fn prepare_panel_window(xdg_surface: &Rc<XdgSurface>, toplevel: &Rc<XdgToplevel>) {
    let mut handler = xdg_surface
        .try_get_handler_mut::<ClientXdgSurface>()
        .expect("the held-back handler is installed");
    XdgSurfaceHandler::handle_get_toplevel(&mut *handler, xdg_surface, toplevel);
    XdgSurfaceHandler::handle_set_window_geometry(&mut *handler, xdg_surface, 0, 0, 480, 1080);
}

/// A popup of `parent` and its own surface, deferred through the parent's
/// panel handler: the popup's surface and positioner are minted without a
/// compositor, so nothing is sent.
fn popup_of(
    state: &Rc<State>,
    routing: &Rc<Routing>,
    parent: &Rc<XdgSurface>,
) -> (Rc<XdgPopup>, Rc<XdgPositioner>) {
    let popup_surface = state.create_object::<WlSurface>(6);
    let popup_xdg_surface = state.create_object::<XdgSurface>(1);
    install_held_back(state, routing, &popup_surface, &popup_xdg_surface);
    let popup = state.create_object::<XdgPopup>(1);
    let positioner = state.create_object::<XdgPositioner>(1);
    positioner.set_handler(ClientXdgPositioner::new(&positioner));
    let mut handler = popup_xdg_surface
        .try_get_handler_mut::<ClientXdgSurface>()
        .expect("the held-back handler is installed");
    XdgSurfaceHandler::handle_get_popup(
        &mut *handler,
        &popup_xdg_surface,
        &popup,
        Some(parent),
        &positioner,
    );
    (popup, positioner)
}

/// A popup the app created while the panel's first dock waited is routed
/// once the dock resolves instead of being dropped as a popup of a
/// surface without an upstream parent (D9): it becomes a popup of the
/// panel's layer surface.
#[test]
fn a_popup_of_a_waiting_panel_docks_with_the_panel() {
    let state = state();
    let routing = Routing::new(PanelSettings::built_in());
    // Two outputs leave the compositor's choice open, so the first dock
    // waits for the panel's own output (D9).
    routing
        .globals()
        .install_test_output(10, output(2560, 1440));
    routing
        .globals()
        .install_test_output(11, output(1920, 1080));
    let layer_shell = state.create_object::<ZwlrLayerShellV1>(5);
    routing.set_layer_shell(layer_shell);

    let (_surface, xdg_surface) = panel_window(&state, &routing);
    assert!(
        routing.pending_snapshot().is_some(),
        "the first dock waits for its output size"
    );
    let (popup, _positioner) = popup_of(&state, &routing, &xdg_surface);
    assert!(
        popup.try_get_handler_ref::<ClientXdgPopup>().is_err(),
        "a popup of a waiting panel is not created yet"
    );

    resume(&xdg_surface, &routing);

    assert!(routing.has_window(), "the wait's fallback docked the panel");
    assert_eq!(
        routing.dock_output(),
        Some(output(2560, 1440)),
        "the dock keeps the size it resolved against"
    );
    assert!(
        popup.try_get_handler_ref::<ClientXdgPopup>().is_ok(),
        "the deferred popup became a popup of the layer surface"
    );
    assert!(
        xdg_surface
            .try_get_handler_ref::<ClientXdgSurface>()
            .expect("the panel handler is installed")
            .pending_popups
            .is_empty(),
        "the routed popup left the wait"
    );
}

/// A popup of a waiting panel whose wait forwards the window takes the
/// forwarded branch too (D9), so it is not dropped when the panel never
/// docks.
#[test]
fn a_popup_of_a_forwarded_panel_keeps_its_parent() {
    let state = state();
    let routing = Routing::new(PanelSettings::built_in());
    let layer_shell = state.create_object::<ZwlrLayerShellV1>(5);
    routing.set_layer_shell(layer_shell);

    let (_surface, xdg_surface) = panel_window(&state, &routing);
    let (popup, _positioner) = popup_of(&state, &routing, &xdg_surface);

    resume(&xdg_surface, &routing);

    assert!(!routing.has_window(), "no output reported a size");
    assert!(
        popup.try_get_handler_ref::<ClientXdgPopup>().is_err(),
        "a forwarded popup keeps its own coordinates"
    );
    assert!(
        xdg_surface
            .try_get_handler_ref::<ClientXdgSurface>()
            .expect("the panel handler is installed")
            .pending_popups
            .is_empty(),
        "the routed popup left the wait"
    );
}

/// A positioner the app destroys right after `get_popup` stays upstream until
/// the waiting dock routes the popup (D9): a popup cannot be created from a
/// positioner whose upstream object is gone, so the proxy finishes the
/// `destroy` after the popup.
#[test]
fn a_positioner_destroy_waits_for_the_waiting_popup() {
    let state = state();
    let routing = Routing::new(PanelSettings::built_in());
    // Two outputs leave the compositor's choice open, so the first dock
    // waits for the panel's own output (D9).
    routing
        .globals()
        .install_test_output(10, output(2560, 1440));
    routing
        .globals()
        .install_test_output(11, output(1920, 1080));
    let layer_shell = state.create_object::<ZwlrLayerShellV1>(5);
    routing.set_layer_shell(layer_shell);
    let (_surface, xdg_surface) = panel_window(&state, &routing);
    let (popup, positioner) = popup_of(&state, &routing, &xdg_surface);

    destroy_positioner(&positioner);

    resume(&xdg_surface, &routing);

    assert!(
        popup.try_get_handler_ref::<ClientXdgPopup>().is_ok(),
        "the popup was created for the positioner"
    );
    assert!(
        positioner
            .try_get_handler_ref::<ClientXdgPositioner>()
            .is_err(),
        "the held destroy went out once the popup existed"
    );
}
