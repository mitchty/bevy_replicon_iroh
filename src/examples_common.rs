//! Just common stuff for the echo server/client examples, nothing special. In
//! here and not in say examples/common as type_name() is used to construct a
//! protocol hash in `bevy_replicon::shared::protocol::ProtocolHasher`. I should
//! probably do something better than keep this in all shipped code but this is
//! an MVP and worthy enough to ship for other crazy souls to try. A bit of code
//! bloats ok for now.

// TODO: Better option for replicated types across disparate binaries. Maybe
// macro magique!? Probably not it angers rust programmers but this is "future
// mitch" problem.
//
// But since each example crate is its own binary I was getting protocol
// mismatch between the client and server due to the protocol mismatch between
// the underlying types for the example code. Note this isn't needed if its the
// same binary but I wanted to prove out literally distinct client/server code
// not a shared binary.
//
// I'll probably migrate this to its own workspace example crate in future so
// that this code isn't present normally. But I started this out as a single
// crate and thats more effort than I want to expend today.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Trivial replicated counter ticked once a second by
/// `echo_server` and observed as read only in `echo_client`.
#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[doc(hidden)]
pub struct Counter(pub u32);

#[doc(hidden)]
pub fn tick(mut counters: Query<&mut Counter>) {
    for mut counter in &mut counters {
        counter.0 += 1;
    }
}
