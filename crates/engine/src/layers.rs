//! Layer (optional content) visibility: what the view shows and what printing writes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::ObjRef;
use pdfcraft_render::{Layer, LayerOp, RenderPool};

use crate::{Document, display_config, render_threads};

/// Write `layers`' visibility into the default optional-content configuration of `cos` (a copy
/// made for printing): `/BaseState /ON`, `/ON` the visible layers, `/OFF` the hidden ones. Its
/// other entries (`/Order`, `/RBGroups`, `/AS`…) are kept. A document without `/OCProperties`
/// is left alone.
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
    properties.set(b"D".to_vec(), Object::Dict(config));
    root.set(b"OCProperties".to_vec(), Object::Dict(properties));
    cos.set(root_ref, Object::Dict(root));
}

/// Render `doc` with the layer visibility in its `info.layers`.
pub(crate) fn use_layer_choices(doc: &mut Document) {
    let overrides: Vec<(i32, i32, bool)> = doc.info.layers.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect();
    doc.config.layers = Arc::new(overrides);
    doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), display_config(&doc.config));
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
            Some(d) if d.name(b"Type") == Some(b"OCMD") => {
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
