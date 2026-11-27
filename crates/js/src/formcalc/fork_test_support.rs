//! Set-up the fork's FormCalc tests share.

use std::collections::HashMap;

use super::Interp;
use crate::xfa::{XHost, XfaEvent};

/// An interpreter over `h` with room for 1,000 loop turns and 32 nested calls.
pub(super) fn interp(h: &mut XHost) -> Interp<'_> {
    Interp {
        h,
        scopes: vec![HashMap::new()],
        funcs: HashMap::new(),
        event: XfaEvent::default(),
        steps: 0,
        loops_left: 1000,
        depth: 0,
        max_depth: 32,
        exited: false,
        #[cfg(not(target_arch = "wasm32"))]
        deadline: None,
        #[cfg(not(target_arch = "wasm32"))]
        out_of_time: false,
    }
}
