//! The panel's one window slot (replace-terminal-with-app-host D4): the
//! empty-slot rule that decides whether a new top-level window becomes the
//! panel or a normal compositor window.
//!
//! The type is pure and display-free (`port-to-rust` D10), so the five spec
//! scenarios — "First window", "Window comes back", "Dialog", "Passed-through
//! window stays" and "Empty panel shows nothing" — are unit tested without a
//! compositor.
//!
//! The rule, stated once: a window takes the slot only while it is empty. A
//! window that arrives while the slot is full passes through and is never
//! stored or promoted, so it stays a normal window when the panel's window
//! later closes. Destroying the slot's own window empties the slot, which is
//! what lets the app's next window take the panel again.

/// What the panel's slot holds (D4): no window, or the one panel window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Slot {
    /// The panel holds no window: nothing of it shows and it reserves no
    /// strip (spec "Empty panel shows nothing").
    #[default]
    Empty,
    /// The panel holds the toplevel named by `window`, the id
    /// [`ObjectCoreApi::unique_id`](wl_proxy::object::ObjectCoreApi::unique_id)
    /// gave it.
    Held {
        /// The unique id of the panel's `xdg_toplevel`.
        window: u64,
    },
}

/// What a window that asked for the toplevel role becomes (D4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The slot was empty and takes the window: it becomes the panel.
    Panel,
    /// The slot was full: the window passes through unchanged and joins no
    /// slot.
    PassedThrough,
}

impl Slot {
    /// An empty slot.
    #[must_use]
    pub const fn new() -> Self {
        Self::Empty
    }

    /// Routes the toplevel `window` (D4): an empty slot takes it as the panel
    /// and a full one passes it through unchanged.
    pub fn route(&mut self, window: u64) -> Route {
        match *self {
            Self::Empty => {
                *self = Self::Held { window };
                Route::Panel
            }
            Self::Held { .. } => Route::PassedThrough,
        }
    }

    /// Releases `window` if it is the slot's own window (D4: destroy of the
    /// slot's toplevel empties the slot). Returns whether the slot emptied;
    /// another window's destroy leaves the slot as it is (spec
    /// "Passed-through window stays").
    pub fn release(&mut self, window: u64) -> bool {
        match *self {
            Self::Held { window: held } if held == window => {
                *self = Self::Empty;
                true
            }
            Self::Empty | Self::Held { .. } => false,
        }
    }

    /// Whether the panel holds a window: the status reply's `has_window`
    /// (spec "Status", "Empty panel shows nothing").
    #[must_use]
    pub const fn has_window(self) -> bool {
        matches!(self, Self::Held { .. })
    }

    /// The held toplevel's id, or `None` while the slot is empty.
    #[must_use]
    pub const fn window(self) -> Option<u64> {
        match self {
            Self::Empty => None,
            Self::Held { window } => Some(window),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Route, Slot};

    /// "First window" (spec "First window"): the first toplevel of an empty
    /// slot becomes the panel.
    #[test]
    fn first_window() {
        let mut slot = Slot::new();
        assert_eq!(slot.route(7), Route::Panel);
        assert_eq!(slot.window(), Some(7));
        assert!(slot.has_window());
    }

    /// "Window comes back" (spec "Window comes back"): after the panel's
    /// window closes, the app's next window takes the slot again.
    #[test]
    fn window_comes_back() {
        let mut slot = Slot::new();
        assert_eq!(slot.route(7), Route::Panel);
        assert!(slot.release(7));
        assert!(!slot.has_window());
        assert_eq!(slot.route(8), Route::Panel);
        assert_eq!(slot.window(), Some(8));
    }

    /// "Dialog" (spec "Dialog"): a second toplevel while the slot is full
    /// passes through and the panel keeps its window.
    #[test]
    fn dialog() {
        let mut slot = Slot::new();
        assert_eq!(slot.route(7), Route::Panel);
        assert_eq!(slot.route(8), Route::PassedThrough);
        assert_eq!(slot.window(), Some(7));
    }

    /// "Passed-through window stays" (spec "Passed-through window stays"):
    /// the passed-through window joins no slot, so closing it leaves the
    /// panel's window held and closing the panel's window does not promote
    /// it.
    #[test]
    fn passed_through_window_stays() {
        let mut slot = Slot::new();
        assert_eq!(slot.route(7), Route::Panel);
        assert_eq!(slot.route(8), Route::PassedThrough);
        assert!(!slot.release(8));
        assert_eq!(slot.window(), Some(7));
        assert!(slot.release(7));
        assert!(!slot.has_window());
        assert!(!slot.release(8));
        assert!(!slot.has_window());
    }

    /// "Empty panel shows nothing" (spec "Empty panel shows nothing"): an
    /// empty slot reports no window, which is the status `has_window` false.
    #[test]
    fn empty_panel_shows_nothing() {
        let mut slot = Slot::new();
        assert!(!slot.has_window());
        assert_eq!(slot.window(), None);
        assert!(!slot.release(7));
        assert!(!slot.has_window());
    }
}
