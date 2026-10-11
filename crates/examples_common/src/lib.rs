//! Shared stuff for the echo examples.
//!
//! Kept in its own crate so that types used to compute `bevy_replicon`'s
//! `ProtocolHash` are identical across the `echo_server` and `echo_client` to
//! prevent protocol-mismatch. No other real reason for the workspace crate.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Trivial replicated counter ticked once a second by `echo_server` and
/// observed as read-only in `echo_client`.
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct Counter(pub u32);

/// Increments every [`Counter`] by one. Run on a timer in `echo_server`.
pub fn tick(mut counters: Query<&mut Counter>) {
    for mut counter in &mut counters {
        counter.0 += 1;
    }
}
