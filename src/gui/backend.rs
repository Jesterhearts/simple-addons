//! Which windowing backend the window opens on.
//!
//! Drag and drop is how this program is meant to be used, and on Linux only
//! winit's X11 backend reports dropped files. Its Wayland backend implements
//! no data device at all, so a window opened on Wayland never hears about a
//! drag, let alone a drop. A Wayland session almost always runs XWayland
//! alongside, so the window asks for X11 whenever an X server answers, and
//! settles for Wayland only when none does.
//!
//! The choice has to be right first time. winit marks an event loop as built
//! before it builds one, failure and all, so a window that will not open on
//! X11 cannot then be opened on Wayland instead; the X server is asked
//! whether it is there before the choice is made rather than after it goes
//! wrong.
//!
//! `SIMPLE_ADDONS_BACKEND` overrides the choice, for anyone who would rather
//! have a native Wayland window and use the buttons instead.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use winit::platform::wayland::EventLoopBuilderExtWayland as _;
use winit::platform::x11::EventLoopBuilderExtX11 as _;

/// Where X servers put the sockets that local clients connect on.
const X_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// The backend winit should open the window on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// X11, which on a Wayland session means XWayland. The only Linux backend
    /// that reports dropped files.
    X11,
    /// Wayland, where dropped files never arrive.
    Wayland,
    /// Whatever winit picks for itself.
    Native,
}

/// What the environment says about the display servers on offer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Session {
    /// The backend named in `SIMPLE_ADDONS_BACKEND`, if one was named.
    pub requested: Option<String>,
    /// A Wayland compositor is named in the environment.
    pub wayland: bool,
    /// An X server answered on the display named in the environment.
    pub x11: bool,
}

/// Reads the session from the environment, asking any X server it names
/// whether it is really there.
pub fn read_session() -> Session {
    let display = set_to_something("DISPLAY");
    Session {
        requested: set_to_something("SIMPLE_ADDONS_BACKEND"),
        wayland: set_to_something("WAYLAND_DISPLAY").is_some()
            || set_to_something("WAYLAND_SOCKET").is_some(),
        x11: display.is_some_and(|display| x_server_answers(Path::new(X_SOCKET_DIR), &display)),
    }
}

/// A variable set to an empty string counts as unset, which is how winit
/// reads these too.
fn set_to_something(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub fn choose(session: &Session) -> Backend {
    match session.requested.as_deref() {
        Some("x11") => Backend::X11,
        Some("wayland") => Backend::Wayland,
        // A name that means nothing is not worth refusing to open a window
        // over, so it falls through to the usual choice.
        _ => preferred(session),
    }
}

/// X11 wins wherever it is on offer, because drops only arrive there.
fn preferred(session: &Session) -> Backend {
    match (session.x11, session.wayland) {
        (true, _) => Backend::X11,
        (false, true) => Backend::Wayland,
        // Nowhere to open a window. Leave winit to say so in its own words.
        (false, false) => Backend::Native,
    }
}

/// Whether a window on this backend will ever see a dropped file, which is
/// what the drop zone needs to know before inviting a drop.
pub fn reports_drops(backend: Backend) -> bool {
    !matches!(backend, Backend::Wayland)
}

/// The hook that makes winit open on `backend`, or none when winit's own
/// choice is the one wanted.
pub fn force(backend: Backend) -> Option<eframe::EventLoopBuilderHook> {
    match backend {
        Backend::X11 => Some(Box::new(|builder| {
            builder.with_x11();
        })),
        Backend::Wayland => Some(Box::new(|builder| {
            builder.with_wayland();
        })),
        Backend::Native => None,
    }
}

/// What to suggest when a window would not open on this backend.
///
/// Only asking for X11 is worth explaining: it is the one case where this
/// program overrules what winit would have done by itself, so a user left
/// without a window deserves to hear that, and to hear how to undo it.
pub fn advice(backend: Backend) -> Option<&'static str> {
    match backend {
        Backend::X11 => Some(
            "drag and drop works only on X11, so the window asked for it; \
             SIMPLE_ADDONS_BACKEND=wayland opens the window without drag and drop",
        ),
        Backend::Wayland | Backend::Native => None,
    }
}

/// Where a `DISPLAY` value says an X server is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum XDisplay {
    /// On this machine, listening on the socket for this display number.
    Local(String),
    /// Somewhere on the network, which cannot be checked without talking X.
    Network,
    /// Not a display address at all.
    Nowhere,
}

/// Whether an X server answers on `display`.
///
/// A local server is a unix socket, so connecting to it settles the matter. A
/// server reached over the network is taken at its word, since checking means
/// speaking the protocol.
fn x_server_answers(socket_dir: &Path, display: &str) -> bool {
    match read_display(display) {
        XDisplay::Local(number) => UnixStream::connect(socket_path(socket_dir, &number)).is_ok(),
        XDisplay::Network => true,
        XDisplay::Nowhere => false,
    }
}

fn socket_path(socket_dir: &Path, number: &str) -> PathBuf {
    socket_dir.join(format!("X{number}"))
}

/// Reads a `DISPLAY` value, which is `host:number.screen` with the host left
/// out, or written `unix`, for a server on this machine.
fn read_display(display: &str) -> XDisplay {
    let Some((host, tail)) = display.rsplit_once(':') else {
        return XDisplay::Nowhere;
    };
    let Some(number) = display_number(tail) else {
        return XDisplay::Nowhere;
    };
    match host.trim_end_matches('/') {
        "" | "unix" => XDisplay::Local(number.to_string()),
        _ => XDisplay::Network,
    }
}

/// The display number, which is the digits before any screen number.
fn display_number(tail: &str) -> Option<&str> {
    let number = tail.split('.').next()?;
    let digits = !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
    digits.then_some(number)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    fn session(wayland: bool, x11: bool) -> Session {
        Session {
            requested: None,
            wayland,
            x11,
        }
    }

    #[test]
    fn a_wayland_session_running_xwayland_opens_on_x11_so_drops_arrive() {
        assert_eq!(choose(&session(true, true)), Backend::X11);
    }

    #[test]
    fn a_plain_x11_session_opens_on_x11() {
        assert_eq!(choose(&session(false, true)), Backend::X11);
    }

    #[test]
    fn a_wayland_session_without_xwayland_has_to_make_do() {
        let chosen = choose(&session(true, false));

        assert_eq!(chosen, Backend::Wayland);
        assert!(!reports_drops(chosen));
    }

    #[test]
    fn nowhere_to_open_leaves_the_choice_to_winit() {
        assert_eq!(choose(&session(false, false)), Backend::Native);
    }

    #[test]
    fn a_requested_backend_beats_what_the_session_offers() {
        let requested = |name: &str| Session {
            requested: Some(name.to_string()),
            wayland: true,
            x11: true,
        };

        assert_eq!(choose(&requested("wayland")), Backend::Wayland);
        assert_eq!(choose(&requested("x11")), Backend::X11);
    }

    #[test]
    fn a_nonsense_backend_falls_back_to_the_usual_choice() {
        let session = Session {
            requested: Some("mir".to_string()),
            wayland: true,
            x11: true,
        };

        assert_eq!(choose(&session), Backend::X11);
    }

    #[test]
    fn only_wayland_fails_to_report_drops() {
        assert!(reports_drops(Backend::X11));
        assert!(reports_drops(Backend::Native));
        assert!(!reports_drops(Backend::Wayland));
    }

    #[test]
    fn only_a_named_backend_needs_a_hook() {
        assert!(force(Backend::X11).is_some());
        assert!(force(Backend::Wayland).is_some());
        assert!(force(Backend::Native).is_none());
    }

    #[test]
    fn asking_for_x11_is_the_choice_worth_explaining() {
        assert!(advice(Backend::X11).is_some());
        assert!(advice(Backend::Wayland).is_none());
        assert!(advice(Backend::Native).is_none());
    }

    #[test]
    fn a_display_without_a_host_is_a_socket_on_this_machine() {
        assert_eq!(read_display(":0"), XDisplay::Local("0".to_string()));
        assert_eq!(read_display(":12"), XDisplay::Local("12".to_string()));
    }

    #[test]
    fn a_screen_number_is_not_part_of_the_display_number() {
        assert_eq!(read_display(":1.0"), XDisplay::Local("1".to_string()));
    }

    #[test]
    fn a_display_written_the_long_way_round_is_still_a_socket() {
        assert_eq!(read_display("unix:0"), XDisplay::Local("0".to_string()));
        assert_eq!(read_display("unix/:0"), XDisplay::Local("0".to_string()));
    }

    #[test]
    fn a_display_on_another_machine_is_on_the_network() {
        assert_eq!(read_display("otherbox:0"), XDisplay::Network);
        assert_eq!(read_display("localhost:10.0"), XDisplay::Network);
    }

    #[test]
    fn a_display_that_names_no_number_is_nowhere() {
        assert_eq!(read_display(""), XDisplay::Nowhere);
        assert_eq!(read_display("wayland-0"), XDisplay::Nowhere);
        assert_eq!(read_display(":"), XDisplay::Nowhere);
        assert_eq!(read_display(":first"), XDisplay::Nowhere);
    }

    #[test]
    fn a_local_display_answers_only_while_something_is_listening() {
        let dir = tempfile::tempdir().unwrap();
        let listening = UnixListener::bind(socket_path(dir.path(), "7")).unwrap();

        assert!(x_server_answers(dir.path(), ":7"));
        assert!(!x_server_answers(dir.path(), ":8"));

        drop(listening);
        assert!(!x_server_answers(dir.path(), ":7"));
    }

    #[test]
    fn a_display_on_the_network_is_taken_at_its_word() {
        let dir = tempfile::tempdir().unwrap();

        assert!(x_server_answers(dir.path(), "otherbox:0"));
    }

    #[test]
    fn a_display_that_is_not_an_address_answers_for_nothing() {
        let dir = tempfile::tempdir().unwrap();

        assert!(!x_server_answers(dir.path(), "nonsense"));
    }
}
