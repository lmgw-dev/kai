//! Client bindings for the two Plasma protocols the probes use, generated from the XML files in
//! `protocols/` (copied from plasma-wayland-protocols; the system has no -devel package).

pub mod pwm {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports, missing_docs, clippy::all)]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/plasma-window-management.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/plasma-window-management.xml");
}

pub mod fake {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports, missing_docs, clippy::all)]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/fake-input.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/fake-input.xml");
}
