//! Client bindings, generated from the protocol files beside this module: the window management
//! protocol (plasma-wayland-protocols, as WP13 used it) and xdg-output (the outputs' names and
//! logical geometry, which `wl_output` alone does not give under a fractional scale), and KWin's
//! fake input (WP16c).

pub mod wm {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(
        non_upper_case_globals,
        non_snake_case,
        unused_imports,
        missing_docs,
        clippy::all
    )]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!(
            "src/desktop/plasma/protocols/plasma-window-management.xml"
        );
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!(
        "src/desktop/plasma/protocols/plasma-window-management.xml"
    );
}

pub mod xdg_output {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(
        non_upper_case_globals,
        non_snake_case,
        unused_imports,
        missing_docs,
        clippy::all
    )]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!(
            "src/desktop/plasma/protocols/xdg-output-unstable-v1.xml"
        );
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!(
        "src/desktop/plasma/protocols/xdg-output-unstable-v1.xml"
    );
}

/// KWin's fake input (WP16c, `type_text` and `key_press`): the restricted interface that KWin
/// grants per `.desktop` file (`X-KDE-Wayland-Interfaces`, ruling 16). The copy is plasma-wayland-
/// protocols' `fake-input.xml`, version 6 (`keyboard_keysym`).
pub mod fake_input {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(
        non_upper_case_globals,
        non_snake_case,
        unused_imports,
        missing_docs,
        clippy::all
    )]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("src/desktop/plasma/protocols/fake-input.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("src/desktop/plasma/protocols/fake-input.xml");
}
