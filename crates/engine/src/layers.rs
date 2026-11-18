//! Layer (optional content) visibility: what the view shows and what printing writes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::ObjRef;
use pdfcraft_render::{Layer, LayerOp, RenderPool};

use crate::{Document, render_threads};

/// Most usage application dictionaries (`/AS` entries) that [`print_layer_states`] reads, and
/// most category names and groups it reads in all of them together (entries can share one
/// array, so the count is over every entry, not each).
const MAX_AUTO_STATES: usize = 1024;
const MAX_AUTO_ITEMS: usize = 65_536;

/// The usage dictionary entry and its state key that a `/Category` name of a usage application
/// dictionary reads when printing (ISO 32000-2 Table 101): Print, View and Export each name a
/// state of the group's own. Zoom, User and Language depend on the magnification, the user and
/// the system language, which printing doesn't fix, so they give no recommendation here.
fn print_category(name: &[u8]) -> Option<(&'static [u8], &'static [u8])> {
    match name {
        b"Print" => Some((b"Print", b"PrintState")),
        b"View" => Some((b"View", b"ViewState")),
        b"Export" => Some((b"Export", b"ExportState")),
        _ => None,
    }
}

/// The layers as they print: `layers` as shown, then the default configuration's auto state for
/// the Print event (ISO 32000-2 §8.11.4.4, Table 101). Each usage application dictionary
/// (`/AS` entry) whose `/Event` is `/Print` recommends, for each group in its `/OCGs` and each of
/// its categories ([`print_category`]), the state the group's usage dictionary gives (for the
/// Print category, `/Usage /Print /PrintState`). A group with recommendations is on only if all
/// of them, in all the entries naming it, are ON; other groups keep their state. `/Category` is
/// an array; a single name is read as one. `None` when no `/AS` entry is for printing.
pub(crate) fn print_layer_states(cos: &pdfcraft_cos::Document, layers: &[Layer]) -> Option<Vec<Layer>> {
    use pdfcraft_cos::Object;
    let root = cos.trailer().get(b"Root").map(|r| cos.resolve(r))?;
    let properties = root.as_dict()?.get(b"OCProperties").map(|p| cos.resolve(p))?;
    let config = properties.as_dict()?.get(b"D").map(|d| cos.resolve(d))?;
    let auto = config.as_dict()?.get(b"AS").map(|a| cos.resolve(a))?;
    let mut printing = false;
    let mut recommended: std::collections::HashMap<(u32, u16), bool> = std::collections::HashMap::new();
    let mut items_left = MAX_AUTO_ITEMS;
    for entry in auto.as_array()?.iter().take(MAX_AUTO_STATES) {
        let entry = cos.resolve(entry);
        let Some(entry) = entry.as_dict() else { continue };
        if entry.get(b"Event").map(|e| cos.resolve(e)).as_deref().and_then(Object::as_name) != Some(b"Print") {
            continue;
        }
        printing = true;
        let categories = entry.get(b"Category").map(|c| cos.resolve(c));
        let mut read: Vec<(&[u8], &[u8])> = Vec::new();
        let mut consider = |name: Option<&[u8]>| {
            if let Some(category) = name.and_then(print_category).filter(|c| !read.contains(c)) {
                read.push(category);
            }
        };
        match categories.as_deref() {
            Some(Object::Array(names)) => {
                let count = names.len().min(items_left);
                items_left -= count;
                names.iter().take(count).for_each(|n| consider(cos.resolve(n).as_name()));
            }
            Some(Object::Name(n)) => consider(Some(n.as_slice())),
            _ => {}
        }
        if read.is_empty() {
            continue;
        }
        let Some(groups) = entry.get(b"OCGs").map(|g| cos.resolve(g)) else { continue };
        for group in groups.as_array().into_iter().flatten() {
            let Some(left) = items_left.checked_sub(1) else { break };
            items_left = left;
            let Some(id) = group.as_ref() else { continue };
            let ocg = cos.resolve(group);
            let usage = ocg.as_dict().and_then(|g| g.get(b"Usage").map(|u| cos.resolve(u)));
            for (entry_key, state_key) in &read {
                // No state given (a Print category without `/PrintState`): no recommendation, so
                // the group is left as it is.
                let state = usage
                    .as_deref()
                    .and_then(|u| u.as_dict().and_then(|u| u.get(entry_key).map(|e| cos.resolve(e))))
                    .and_then(|e| e.as_dict().and_then(|e| e.get(state_key).map(|s| cos.resolve(s))))
                    .and_then(|s| match s.as_name() {
                        Some(b"ON") => Some(true),
                        Some(b"OFF") => Some(false),
                        _ => None,
                    });
                if let Some(on) = state {
                    let all_on = recommended.entry((id.num, id.generation)).or_insert(true);
                    *all_on = *all_on && on;
                }
            }
        }
    }
    printing.then(|| layers.iter().map(|l| Layer { visible: recommended.get(&l.id).copied().unwrap_or(l.visible), ..l.clone() }).collect())
}

/// Write `layers`' visibility into the default optional-content configuration of `cos` (a copy
/// made for printing): `/BaseState /ON`, `/ON` the visible layers, `/OFF` the hidden ones. The
/// states written are final, so what would set them again goes: the auto state (`/AS`), and the
/// layers' own `/Usage`, which some readers apply when they draw (a `/ViewState /ON` turns a layer
/// that prints off back on). The configuration's other entries (`/Order`, `/RBGroups`…) are kept.
/// A document without `/OCProperties` is left alone.
pub(crate) fn set_default_layer_state(cos: &mut pdfcraft_cos::Document, layers: &[Layer]) {
    use pdfcraft_cos::{Dict, ObjRef, Object};
    let Some(root_ref) = cos.trailer().get(b"Root").and_then(Object::as_ref) else { return };
    let Some(mut root) = cos.get(root_ref).as_dict().cloned() else { return };
    let Some(properties) = root.get(b"OCProperties").cloned() else { return };
    let Some(mut properties) = cos.resolve(&properties).as_dict().cloned() else { return };
    let mut config = properties.get(b"D").and_then(|d| cos.resolve(d).as_dict().cloned()).unwrap_or_else(Dict::new);
    let refs =
        |visible: bool| Object::Array(layers.iter().filter(|l| l.visible == visible).map(|l| Object::Ref(ObjRef::new(l.id.0, l.id.1))).collect());
    config.set(b"BaseState".to_vec(), Object::name("ON"));
    config.set(b"ON".to_vec(), refs(true));
    config.set(b"OFF".to_vec(), refs(false));
    config.remove(b"AS");
    properties.set(b"D".to_vec(), Object::Dict(config));
    root.set(b"OCProperties".to_vec(), Object::Dict(properties));
    cos.set(root_ref, Object::Dict(root));
    for layer in layers {
        let id = ObjRef::new(layer.id.0, layer.id.1);
        let Some(mut group) = cos.get(id).as_dict().cloned() else { continue };
        if group.remove(b"Usage").is_some() {
            cos.set(id, Object::Dict(group));
        }
    }
}

/// Render `doc` with the layer visibility in its `info.layers`.
pub(crate) fn use_layer_choices(doc: &mut Document) {
    let overrides: Vec<(i32, i32, bool)> = doc.info.layers.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect();
    doc.config.layers = Arc::new(overrides);
    doc.config.print_layers = print_preview_layers(doc.editor.as_ref().map(|e| &e.cos), &doc.info.layers);
    doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), crate::live_scripts::screen_config(doc));
}

/// The layer choices redaction search reads, as `RenderConfig::layers` takes them: the layers as
/// the file opens them, every layer on when one is off as it opens, and every layer off when one
/// is on as it opens. Each is a full extraction, so keep all three. A search then finds content
/// that one group `/OCGs` lists shows, either on or off, and content under any `/P` policy
/// (AllOn, AnyOn, AnyOff, AllOff) over listed groups. Not searched: text that shows only under a
/// mix of layers on and off (a `/VE` expression or nested memberships), and text that a group
/// `/OCGs` does not list hides as the file opens, since such a group keeps its `/D` state.
pub fn redaction_search_layers(layers: &[Layer]) -> Vec<Arc<Vec<(i32, i32, bool)>>> {
    let every = |visible: bool| Arc::new(layers.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, visible)).collect());
    let mut choices = vec![Arc::default()];
    if layers.iter().any(|l| !l.initial) {
        choices.push(every(true));
    }
    if layers.iter().any(|l| l.initial) {
        choices.push(every(false));
    }
    choices
}

/// The optional content the view hides, worked out as the renderer does
/// (`vendor/hayro-interpret/src/ocg.rs`): the default configuration (`/OCProperties /D`), then the
/// view's choice for each layer in `info.layers`.
pub(crate) struct ViewLayers<'a> {
    cos: &'a pdfcraft_cos::Document,
    /// The groups `/OCProperties /OCGs` lists: the only ones a visibility expression may name.
    listed: HashSet<(u32, u16)>,
    /// The groups that are off, listed or not.
    off: HashSet<(u32, u16)>,
    /// Whether each optional content reference met so far hides what it marks.
    known: HashMap<(u32, u16), bool>,
}

impl<'a> ViewLayers<'a> {
    pub(crate) fn new(cos: &'a pdfcraft_cos::Document, layers: &[Layer]) -> Self {
        use pdfcraft_cos::Object;
        let groups = |o: Option<&Object>| -> Vec<(u32, u16)> {
            o.map(|o| cos.resolve(o))
                .as_deref()
                .and_then(Object::as_array)
                .into_iter()
                .flatten()
                .filter_map(Object::as_ref)
                .map(|r| (r.num, r.generation))
                .collect()
        };
        let properties = cos.trailer().get(b"Root").and_then(|r| cos.dict(r)).and_then(|c| c.get(b"OCProperties").and_then(|p| cos.dict(p)));
        let config = properties.as_ref().and_then(|p| p.get(b"D")).and_then(|d| cos.dict(d));
        let (mut listed, mut off) = (HashSet::new(), HashSet::new());
        if let (Some(properties), Some(config)) = (&properties, &config) {
            listed.extend(groups(properties.get(b"OCGs")));
            if config.get(b"BaseState").map(|b| cos.resolve(b)).as_deref().and_then(Object::as_name) == Some(b"OFF") {
                off.extend(listed.iter().copied());
            }
            for id in groups(config.get(b"ON")) {
                off.remove(&id);
            }
            off.extend(groups(config.get(b"OFF")));
        }
        for l in layers {
            if l.visible {
                off.remove(&l.id);
            } else {
                off.insert(l.id);
            }
        }
        ViewLayers { cos, listed, off, known: HashMap::new() }
    }

    /// Whether the view hides what optional content `oc` marks: a group that is off, or a
    /// membership dictionary whose visibility expression (`/VE`), when well formed, or else `/P`
    /// policy (AnyOn when absent or unknown) over the groups `/OCGs` refers to, says so. One with
    /// no groups has no effect.
    pub(crate) fn hides(&mut self, oc: ObjRef) -> bool {
        use pdfcraft_cos::Object;
        let id = (oc.num, oc.generation);
        if let Some(&hidden) = self.known.get(&id) {
            return hidden;
        }
        let is_off = |o: &Object| o.as_ref().is_some_and(|r| self.off.contains(&(r.num, r.generation)));
        let resolved = self.cos.resolve(&Object::Ref(oc));
        let hidden = match resolved.as_dict() {
            Some(d) if d.get(b"Type").map(|ty| self.cos.resolve(ty)).as_deref().and_then(Object::as_name) == Some(b"OCMD") => {
                match d.get(b"VE").and_then(|ve| self.cos.resolve(ve).as_array().and_then(|e| self.expression_shown(e, 0))) {
                    Some(shown) => !shown,
                    None => {
                        // Whether each group it refers to is off.
                        let groups: Vec<bool> = match d.get(b"OCGs") {
                            Some(g) => match self.cos.resolve(g).as_array() {
                                Some(list) => list.iter().filter_map(|o| o.as_ref().map(|_| is_off(o))).collect(),
                                None => g.as_ref().map(|_| is_off(g)).into_iter().collect(),
                            },
                            None => Vec::new(),
                        };
                        let off = groups.iter().filter(|off| **off).count();
                        !groups.is_empty()
                            && match d.get(b"P").map(|p| self.cos.resolve(p)).as_deref().and_then(Object::as_name) {
                                Some(b"AllOn") => off > 0,
                                Some(b"AnyOff") => off == 0,
                                Some(b"AllOff") => off < groups.len(),
                                _ => off == groups.len(),
                            }
                    }
                }
            }
            _ => self.off.contains(&id),
        };
        self.known.insert(id, hidden);
        hidden
    }

    /// Whether visibility expression `e` (`[/And e…]`, `[/Or e…]` or `[/Not e]`, each operand a
    /// group by reference or a nested expression) shows what it marks: `None` when it is malformed
    /// (an unknown operator or operand, a group `/OCGs` doesn't list, nesting past 32), which
    /// leaves `/P` to decide. Nested expressions are direct objects, so the walk is as long as the
    /// expression.
    fn expression_shown(&self, e: &[pdfcraft_cos::Object], depth: usize) -> Option<bool> {
        use pdfcraft_cos::Object;
        let (Some(Object::Name(operator)), false) = (e.first(), depth > 32) else { return None };
        let (mut count, mut all, mut any) = (0usize, true, false);
        for operand in e.get(1..).unwrap_or_default() {
            let shown = match operand {
                Object::Ref(r) if self.listed.contains(&(r.num, r.generation)) => !self.off.contains(&(r.num, r.generation)),
                Object::Array(nested) => self.expression_shown(nested, depth.saturating_add(1))?,
                _ => return None,
            };
            count = count.saturating_add(1);
            all &= shown;
            any |= shown;
        }
        match (operator.as_slice(), count) {
            (b"And", 1..) => Some(all),
            (b"Or", 1..) => Some(any),
            (b"Not", 1) => Some(!all),
            _ => None,
        }
    }
}

/// The layer overrides a print preview draws with (`RenderConfig::print_layers`): the layers as
/// they print ([`print_layer_states`]), so the preview shows the page as it prints. `None` when
/// the document has no auto state for printing (or no structure to read it from).
pub(crate) fn print_preview_layers(cos: Option<&pdfcraft_cos::Document>, layers: &[Layer]) -> Option<Arc<Vec<(i32, i32, bool)>>> {
    let printed = print_layer_states(cos?, layers)?;
    Some(Arc::new(printed.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect()))
}

/// Apply a set-layer-visibility action to `layers`, one change at a time, so a toggle flips the
/// state the changes before it left. Returns whether any layer ended up changed.
pub(crate) fn apply_layer_state(layers: &mut [Layer], groups: &[Vec<(u32, u16)>], changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) -> bool {
    let before: Vec<bool> = layers.iter().map(|l| l.visible).collect();
    let index: std::collections::HashMap<(u32, u16), usize> = layers.iter().enumerate().map(|(i, l)| (l.id, i)).collect();
    for &(op, ocg) in changes {
        let Some(layer) = index.get(&ocg).and_then(|&i| layers.get_mut(i)) else { continue };
        let on = match op {
            LayerOp::On => true,
            LayerOp::Off => false,
            LayerOp::Toggle => !layer.visible,
        };
        layer.visible = on;
        // Turning a layer off leaves the rest of its groups alone.
        if on && preserve_rb {
            for other in groups.iter().filter(|g| g.contains(&ocg)).flatten().filter(|&&o| o != ocg) {
                if let Some(l) = index.get(other).and_then(|&i| layers.get_mut(i)) {
                    l.visible = false;
                }
            }
        }
    }
    layers.iter().zip(before).any(|(l, was)| l.visible != was)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_searches_all_on_or_all_off_only_when_that_differs_from_the_file() {
        let layer = |n: u32, initial: bool| Layer { id: (n, 0), name: n.to_string(), visible: initial, initial };
        let passes =
            |layers: &[Layer]| -> Vec<Vec<bool>> { redaction_search_layers(layers).iter().map(|c| c.iter().map(|&(_, _, v)| v).collect()).collect() };
        assert_eq!(passes(&[]), [Vec::<bool>::new()]);
        assert_eq!(passes(&[layer(1, true), layer(2, true)]), [vec![], vec![false, false]]);
        assert_eq!(passes(&[layer(1, false), layer(2, false)]), [vec![], vec![true, true]]);
        assert_eq!(passes(&[layer(1, true), layer(2, false)]), [vec![], vec![true, true], vec![false, false]]);
    }
}
