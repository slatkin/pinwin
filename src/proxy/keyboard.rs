//! Keyboard modes for a live panel (replace-terminal-with-app-host row 8.3,
//! spec "Keyboard focus by clicking", "Show takes focus", "Show in none
//! mode"): the `set-keyboard` and `focus` commands, and the launch rule that
//! keeps an `on-demand` panel from stealing the keyboard when it opens.
//!
//! Layer shell has no request that grants keyboard focus to a mapped
//! surface. The mode itself is the mechanism (D4): a layer surface that maps
//! with `on_demand` gets the keyboard from a compositor that focuses a newly
//! mapped on-demand surface, `exclusive` keeps it while the surface maps,
//! and `none` never takes it. So `show` (row 8.2) already gives the keyboard
//! when the settings' mode allows it (spec "Show takes focus") and does not
//! when the mode is `none` (spec "Show in none mode"): the show path in
//! [`super::hide`] re-sends the mode before the mapping commit. `focus`
//! on a hidden panel is that same show, and its answer is the reply a show
//! gives.
//!
//! The launch has one extra rule (spec "No focus steal on launch"): an
//! `on-demand` panel maps without the keyboard and gets the mode with the
//! commit after its first buffer, so the first map never takes focus from
//! the window that had it. [`deferred_until_map`] decides the launch, and
//! [`super::commit::PanelSurfaceCommits`] sends the mode after the first
//! forwarded buffer (spec "Click to type" then works as usual). An
//! `exclusive` launch maps as `exclusive` (spec "Opt-in keyboard focus"),
//! and a `none` launch never asks for the keyboard.
//!
//! `set-keyboard` stores the mode in the panel's live settings and re-sends
//! it on the layer surface the window already docks with, through the commit
//! coordinator (D7). It never re-docks or resizes the app (spec "Apply layout
//! without restarting the app") and publishes no event. A later re-dock
//! (`set-output`, `closed`, `set-layer`, see [`super::dock`]) reads the same
//! settings, so it keeps the mode.
//!
//! The launch and mode mapping is display-free, so the tests run without a
//! compositor (`port-to-rust` D10).

use std::rc::Rc;

use wl_proxy::object::ObjectUtils;
use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1KeyboardInteractivity;

use super::commit::PanelSurfaceCommits;
use super::hide;
use super::mapping::keyboard_of;
use super::routing::Routing;
use crate::config::Keyboard;
use crate::daemon::events::Bus;
use crate::daemon::panel::{PanelReply, PanelRequest};
use crate::name::PanelName;

/// The interactivity a layer surface is created with (row 8.3): while a
/// launch's deferral is pending the surface maps without the keyboard, and
/// once it is consumed the surface carries the real mode at once (spec "No
/// focus steal on launch"). [`deferred_until_map`] gives a launch's pending
/// value, and a re-dock reads it from the panel surface's
/// [`super::commit::PanelSurfaceCommits`], so a re-dock that lands before the
/// app's first buffer keeps deferring and a later one does not.
pub(super) const fn dock_interactivity(
    keyboard: Keyboard,
    deferral_pending: bool,
) -> ZwlrLayerSurfaceV1KeyboardInteractivity {
    if deferral_pending {
        ZwlrLayerSurfaceV1KeyboardInteractivity::NONE
    } else {
        keyboard_of(keyboard)
    }
}

/// Whether the launch withholds the keyboard mode until the first buffer
/// maps the surface (spec "No focus steal on launch"): only `on-demand`
/// does, because a compositor focuses a newly mapped on-demand layer
/// surface.
pub(super) const fn deferred_until_map(keyboard: Keyboard) -> bool {
    matches!(keyboard, Keyboard::OnDemand)
}

/// Whether the mode gives the panel the keyboard when its surface maps
/// (spec "Show takes focus", "Show in none mode"): `on-demand` and
/// `exclusive` do, `none` never does.
pub(super) const fn takes_keyboard_when_mapped(keyboard: Keyboard) -> bool {
    !matches!(keyboard, Keyboard::None)
}

/// The answer to `set-keyboard` and `focus` (spec "Panel commands"):
/// `None` for every other request, which the caller answers.
pub(super) fn answer(
    routing: &Rc<Routing>,
    request: &PanelRequest,
    panel: &PanelName,
    bus: &Bus,
) -> Option<PanelReply> {
    match request {
        PanelRequest::SetKeyboard(keyboard) => Some(set(routing, *keyboard)),
        PanelRequest::Focus => Some(focus(routing, panel, bus)),
        _ => None,
    }
}

/// Changes the panel's keyboard mode at run time (spec "Change mode at run
/// time"): the live settings keep it for every later re-dock and status
/// reply, and the layer surface the window already docks with re-sends it
/// through the commit coordinator (D7). The app keeps its window, its size
/// and its process.
fn set(routing: &Rc<Routing>, keyboard: Keyboard) -> PanelReply {
    routing.set_settings(routing.settings().with_keyboard(keyboard));
    if let Some((surface, _, _)) = routing.panel_window()
        && let Ok(commits) = surface.try_get_handler_ref::<PanelSurfaceCommits>()
    {
        // The explicit mode supersedes the launch deferral.
        commits.clear_keyboard_deferral();
        // A hidden panel sends its state on show (D6), so nothing goes out
        // now.
        if !routing.hidden.is_hidden()
            && let Some(layer_surface) = routing.panel_layer_surface()
        {
            commits.request(Box::new(move || {
                layer_surface.send_set_keyboard_interactivity(keyboard_of(keyboard));
            }));
        }
    }
    PanelReply::Done
}

/// Shows a hidden panel and gives it the keyboard when its mode allows it
/// (spec "Focus", "Show takes focus", "Show in none mode"). A shown panel
/// is left alone: layer shell has no request that grants the keyboard to a
/// mapped surface, and only a new map does.
fn focus(routing: &Rc<Routing>, panel: &PanelName, bus: &Bus) -> PanelReply {
    if !routing.hidden.is_hidden() {
        return PanelReply::Done;
    }
    hide::show(routing, panel, bus)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use wl_proxy::protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1KeyboardInteractivity;

    use super::{deferred_until_map, dock_interactivity, takes_keyboard_when_mapped};
    use crate::config::{Keyboard, PanelSettings};
    use crate::daemon::events::{Bus, Event};
    use crate::daemon::panel::{PanelReply, PanelRequest};
    use crate::name::PanelName;
    use crate::proxy::routing::Routing;

    /// A panel name the events name.
    fn notes() -> PanelName {
        PanelName::parse("notes").expect("a panel name")
    }

    /// An `on-demand` launch withholds the mode until the first buffer maps
    /// the surface, so the launch takes no keyboard focus (spec "No focus
    /// steal on launch"); `exclusive` maps as itself (spec "Opt-in keyboard
    /// focus") and `none` never asks for the keyboard.
    #[test]
    fn a_launch_withholds_only_the_on_demand_keyboard() {
        assert_eq!(
            dock_interactivity(Keyboard::OnDemand, deferred_until_map(Keyboard::OnDemand)),
            ZwlrLayerSurfaceV1KeyboardInteractivity::NONE
        );
        assert!(deferred_until_map(Keyboard::OnDemand));
        assert_eq!(
            dock_interactivity(Keyboard::Exclusive, deferred_until_map(Keyboard::Exclusive)),
            ZwlrLayerSurfaceV1KeyboardInteractivity::EXCLUSIVE
        );
        assert!(!deferred_until_map(Keyboard::Exclusive));
        assert_eq!(
            dock_interactivity(Keyboard::None, deferred_until_map(Keyboard::None)),
            ZwlrLayerSurfaceV1KeyboardInteractivity::NONE
        );
        assert!(!deferred_until_map(Keyboard::None));
    }

    /// A re-dock inside the launch-deferral window keeps withholding the
    /// keyboard, so the replacement surface steals no focus, and one after
    /// the first buffer uses the real mode at once (spec "No focus steal on
    /// launch").
    #[test]
    fn a_redock_defers_until_the_launch_deferral_is_consumed() {
        assert_eq!(
            dock_interactivity(Keyboard::OnDemand, true),
            ZwlrLayerSurfaceV1KeyboardInteractivity::NONE,
            "a re-dock before the first buffer carries no keyboard mode"
        );
        assert_eq!(
            dock_interactivity(Keyboard::OnDemand, false),
            ZwlrLayerSurfaceV1KeyboardInteractivity::ON_DEMAND,
            "a launched panel re-docks with its own mode"
        );
        assert_eq!(
            dock_interactivity(Keyboard::Exclusive, false),
            ZwlrLayerSurfaceV1KeyboardInteractivity::EXCLUSIVE
        );
    }

    /// `on-demand` and `exclusive` give the keyboard when a show maps the
    /// panel and `none` does not (spec "Show takes focus", "Show in none
    /// mode").
    #[test]
    fn only_on_demand_and_exclusive_take_the_keyboard_on_show() {
        assert!(takes_keyboard_when_mapped(Keyboard::OnDemand));
        assert!(takes_keyboard_when_mapped(Keyboard::Exclusive));
        assert!(!takes_keyboard_when_mapped(Keyboard::None));
    }

    /// `set-keyboard` stores the mode in the live settings, so a status
    /// reply and the next re-dock keep it (spec "Change mode at run time",
    /// "Apply layout without restarting the app").
    #[test]
    fn set_keyboard_keeps_the_mode_for_a_later_redock() {
        let bus = Bus::new();
        let routing = Routing::new(PanelSettings::built_in());
        let panel = notes();
        assert_eq!(routing.settings().keyboard(), Keyboard::OnDemand);
        assert_eq!(
            super::answer(
                &routing,
                &PanelRequest::SetKeyboard(Keyboard::Exclusive),
                &panel,
                &bus
            ),
            Some(PanelReply::Done)
        );
        assert_eq!(routing.settings().keyboard(), Keyboard::Exclusive);
        assert_eq!(routing.status_settings().keyboard(), Keyboard::Exclusive);
    }

    /// `set-keyboard` publishes no event: it changes the mode in place.
    #[test]
    fn set_keyboard_publishes_no_event() {
        let bus = Bus::new();
        let events = bus.subscribe();
        let routing = Routing::new(PanelSettings::built_in());
        let panel = notes();
        let _ = super::answer(
            &routing,
            &PanelRequest::SetKeyboard(Keyboard::None),
            &panel,
            &bus,
        );
        assert_eq!(
            events.next(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
    }

    /// `focus` on a hidden panel shows it (spec "Focus"), and the show's map
    /// carries the keyboard mode.
    #[test]
    fn focus_shows_a_hidden_panel() {
        let bus = Bus::new();
        let events = bus.subscribe();
        let routing = Routing::new(PanelSettings::built_in());
        let panel = notes();
        routing.hidden.set_hidden(true);
        assert_eq!(
            super::answer(&routing, &PanelRequest::Focus, &panel, &bus),
            Some(PanelReply::Done)
        );
        assert!(!routing.hidden.is_hidden());
        assert_eq!(
            events.next(Duration::from_millis(100)),
            Ok(Event::Shown {
                panel: panel.clone()
            })
        );
    }

    /// `focus` on a shown panel changes nothing: the panel already followed
    /// its mode's focus rules (spec "Click to type"), and no event is
    /// published.
    #[test]
    fn focus_on_a_shown_panel_is_a_no_op() {
        let bus = Bus::new();
        let events = bus.subscribe();
        let routing = Routing::new(PanelSettings::built_in());
        let panel = notes();
        assert_eq!(
            super::answer(&routing, &PanelRequest::Focus, &panel, &bus),
            Some(PanelReply::Done)
        );
        assert!(!routing.hidden.is_hidden());
        assert_eq!(
            events.next(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
    }
}
