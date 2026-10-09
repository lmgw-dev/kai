//! Kai's desktop-neutral half (PD1, K23): the tool contract every hosted tool is declared in
//! ([`contract`]) and the capability traits the desktop providers implement ([`caps`]). It has no
//! desktop, I/O or runtime dependency, so a thread's tool list is the same wherever Kai runs.

pub mod caps;
pub mod contract;
