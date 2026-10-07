//! Serve the instance socket from a library host (`serve-instance-socket`):
//! the bind-then-start opt-in shape from the README — bind an
//! `InstanceSocket` for a name, hand it to the start with
//! `Startup::with_instance`, and show the panel. The start needs a
//! host-owned pty master fd and a Wayland display; `main` passes stdin's fd
//! down so the file compiles, and the start reports its error like any
//! failed start when there is no display.

use std::num::NonZeroU16;
use std::os::fd::RawFd;

use pinwin::Panel;
use pinwin::instance::{InstanceName, InstanceSocket, Request};
use pinwin::layout::{Keyboard, Layout, Side};
use pinwin::panel::Startup;

fn run(fd: RawFd) -> Result<(), Box<dyn std::error::Error>> {
    let name = InstanceName::parse("notes")?;
    // Bind first, before any surface opens: a live owner of the name fails
    // here with the typed duplicate error.
    let socket = InstanceSocket::bind(&name)?;
    let layout = Layout::new(
        Side::Left,
        NonZeroU16::new(60).expect("60 columns is non-zero"),
        0,
        0,
        0,
        12,
    );
    let startup = Startup::new(fd, layout, Keyboard::OnDemand, None).with_instance(socket);
    let panel = Panel::start(startup)?;

    // Show a hidden panel; a shown one stays as it is.
    panel.show()?;
    // The same request from another process is one client call.
    pinwin::instance::send(&name, Request::Show)?;

    drop(panel); // removes the socket file; the name is free at once
    Ok(())
}

fn main() {
    // Stdin's fd stands in for the host-owned pty master; without a display
    // the start fails and the error prints like any failed start.
    if let Err(error) = run(0) {
        eprintln!("instance_host: {error}");
        std::process::exit(1);
    }
}
