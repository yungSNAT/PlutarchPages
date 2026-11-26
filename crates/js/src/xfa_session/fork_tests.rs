//! The fork's tests of the XFA script session.

use std::sync::Arc;
use std::time::Duration;

use super::*;

fn script(source: &str) -> Script {
    Script {
        source: source.into(),
        formcalc: false,
        event: XfaEvent::default(),
        root: Arc::new(XfaNode::default()),
        limits: Limits::default(),
        timeout: Duration::from_secs(30),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_session_starts_one_engine_for_all_its_scripts() {
    let before = engines_started();
    let mut session = XfaSession::new(XfaDoc::default()).with_time_scale(2);
    assert_eq!(session.run(script("1 + 1")).result.as_deref(), Some("2"));
    assert_eq!(session.run(script("2 + 2")).result.as_deref(), Some("4"));
    assert_eq!(engines_started(), before + 1);
}
