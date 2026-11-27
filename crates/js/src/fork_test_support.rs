//! Set-up the fork's tests share: small XFA trees and session jobs.

use std::sync::Arc;

use boa_engine::JsValue;

use crate::host::Host;
use crate::session::Job;
use crate::xfa::{XfaKind, XfaNode};
use crate::{DocInfo, Event};

/// A visible, open field.
pub(crate) fn field(name: &str, som: &str, value: &str, numeric: bool) -> XfaNode {
    XfaNode {
        name: name.into(),
        som: som.into(),
        kind: Some(XfaKind::Field),
        value: value.into(),
        numeric,
        presence: "visible".into(),
        access: "open".into(),
        ..Default::default()
    }
}

/// Instance `index` of a subform that repeats `min` to `max` times.
pub(crate) fn row(name: &str, som: String, index: usize, (min, max): (usize, usize), children: Vec<XfaNode>) -> XfaNode {
    XfaNode {
        name: name.into(),
        som,
        kind: Some(XfaKind::Subform),
        repeatable: true,
        occur_min: min,
        occur_max: Some(max),
        index,
        children,
        ..Default::default()
    }
}

/// The root subform `form1` holding `children`.
pub(crate) fn form1(children: Vec<XfaNode>) -> XfaNode {
    XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children, ..Default::default() }
}

/// A console job running `script` with no fields or document scripts.
pub(crate) fn job(script: &str) -> Job {
    Job {
        script: script.into(),
        event: Event::doc("Console"),
        doc: DocInfo::default(),
        fields: Arc::new(Vec::new()),
        doc_scripts: Vec::new(),
        calculations: Vec::new(),
        timer: None,
    }
}

/// A host with nothing in it and its budgets unspent.
pub(crate) fn bare_host() -> Host {
    Host {
        event_name: String::new(),
        fields: Arc::new(Vec::new()),
        icons: Vec::new(),
        objects: std::collections::HashMap::new(),
        field_keys: std::collections::HashSet::new(),
        changed: std::collections::BTreeSet::new(),
        formatted: std::collections::BTreeSet::new(),
        alerts: Vec::new(),
        console: Vec::new(),
        requests: Vec::new(),
        doc: DocInfo::default(),
        calculations: Vec::new(),
        calculate_depth: 0,
        calculation_evaluations: 0,
        calculation_limit_exceeded: false,
        calculation_work: 0,
        current_event: JsValue::undefined(),
        reset_calls: 0,
        reset_limit_exceeded: false,
        kept: 0,
        retained: 0,
        dropped: 0,
        layers_changed: std::collections::BTreeSet::new(),
    }
}
