//! bo2zm M4: the engine side of Black Ops II's LUI widgets: the element
//! objects `ConstructLUIElement` hands the scripts (`LUI.UIElement.new`),
//! their tree, anchored layout, colour, animation states and timed
//! animations. The widget classes themselves are the game's Lua
//! (ui/lui/*.lua); this is what they call into.
//!
//! An element is userdata whose metatable has `__newindex` = its own field
//! table (the scripts' `setClass` puts the class behind that table) and
//! `__index` = a lookup of that table first, then these natives.
//!
//! Layout: `setLeftRight(leftAnchor, rightAnchor, left, right)` places the
//! left and right edges from the parent's left edge, right edge, or (no
//! anchor) its centre; `setTopBottom` likewise. Animation:
//! `beginAnimation(name, ms, easeIn, easeOut)` starts from the current
//! state and the setters that follow give the end state;
//! `animateToState(name, ms, ...)` ends at a registered state. When one
//! ends the element gets the event `transition_complete_<name>`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::value::{Table, TableRef, UserData, Value};
use crate::vm::{LuaError, Vm};

type Res<T> = Result<T, LuaError>;

/// What an element shows, and where (relative to its parent).
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left_anchor: bool,
    pub top_anchor: bool,
    pub right_anchor: bool,
    pub bottom_anchor: bool,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
    pub alpha_multiplier: f32,
    pub x_rot: f32,
    pub y_rot: f32,
    pub z_rot: f32,
    pub scale: f32,
    /// BO2's `zoom`: the element moves toward the camera by this much, so
    /// it and its children grow about the screen's centre by 1 / (1 - zoom / 1280)
    /// (the focused tile of a grid, GrowingGridButton's button_over).
    pub zoom: f32,
    /// A material name (`RegisterMaterial`), when it draws a picture.
    pub material: Option<String>,
    pub font: Option<String>,
    /// LUI.Alignment: 0 none, 1 left, 2 centre, 3 right (also 4 top,
    /// 5 middle, 6 bottom).
    pub alignment: i32,
    pub text: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left_anchor: false,
            top_anchor: false,
            right_anchor: false,
            bottom_anchor: false,
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
            alpha_multiplier: 1.0,
            x_rot: 0.0,
            y_rot: 0.0,
            z_rot: 0.0,
            scale: 1.0,
            zoom: 0.0,
            material: None,
            font: None,
            alignment: 0,
            text: None,
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl State {
    /// Between two states; pictures, fonts and text switch at the end, the
    /// anchors at the start (the edges carry the motion).
    fn mix(&self, to: &State, t: f32) -> State {
        // bo2mp: an edge pair animated to a negative width (the XP bar's
        // gain, `before + gained - 2` at no gain) ends with no width, not
        // a flipped 2-unit sliver (BO2 draws none: no orange tick).
        let mut to = to.clone();
        if to.left_anchor && !to.right_anchor && to.right < to.left {
            to.right = to.left;
        }
        let to = &to;
        State {
            left: lerp(self.left, to.left, t),
            top: lerp(self.top, to.top, t),
            right: lerp(self.right, to.right, t),
            bottom: lerp(self.bottom, to.bottom, t),
            red: lerp(self.red, to.red, t),
            green: lerp(self.green, to.green, t),
            blue: lerp(self.blue, to.blue, t),
            alpha: lerp(self.alpha, to.alpha, t),
            alpha_multiplier: lerp(self.alpha_multiplier, to.alpha_multiplier, t),
            x_rot: lerp(self.x_rot, to.x_rot, t),
            y_rot: lerp(self.y_rot, to.y_rot, t),
            z_rot: lerp(self.z_rot, to.z_rot, t),
            scale: lerp(self.scale, to.scale, t),
            zoom: lerp(self.zoom, to.zoom, t),
            left_anchor: to.left_anchor,
            top_anchor: to.top_anchor,
            right_anchor: to.right_anchor,
            bottom_anchor: to.bottom_anchor,
            material: if t >= 1.0 { to.material.clone() } else { self.material.clone() },
            font: to.font.clone(),
            alignment: to.alignment,
            text: to.text.clone(),
        }
    }

    /// The fields a registered state table names, over this state.
    fn apply_table(&mut self, t: &Table) {
        let num = |k: &str| t.get_str(k).as_num();
        let flag = |k: &str| match t.get_str(k) {
            Value::Nil => None,
            v => Some(v.truthy()),
        };
        if let Some(v) = num("left") {
            self.left = v;
        }
        if let Some(v) = num("top") {
            self.top = v;
        }
        if let Some(v) = num("right") {
            self.right = v;
        }
        if let Some(v) = num("bottom") {
            self.bottom = v;
        }
        if let Some(v) = flag("leftAnchor") {
            self.left_anchor = v;
        }
        if let Some(v) = flag("topAnchor") {
            self.top_anchor = v;
        }
        if let Some(v) = flag("rightAnchor") {
            self.right_anchor = v;
        }
        if let Some(v) = flag("bottomAnchor") {
            self.bottom_anchor = v;
        }
        if let Some(v) = num("red") {
            self.red = v;
        }
        if let Some(v) = num("green") {
            self.green = v;
        }
        if let Some(v) = num("blue") {
            self.blue = v;
        }
        // A state's `alphaMultiplier` is its alpha by another name: BO2's
        // card carousels are made with `alpha = 0` and shown by a
        // `fade_in` state of `alphaMultiplier = 1` alone (and hidden by
        // `fade_out`'s `alpha = 0`); no script sets the two apart.
        if let Some(v) = num("alphaMultiplier") {
            self.alpha = v;
        }
        if let Some(v) = num("alpha") {
            self.alpha = v;
        }
        if let Some(v) = num("zRot") {
            self.z_rot = v;
        }
        if let Some(v) = num("scale") {
            self.scale = v;
        }
        if let Some(v) = num("zoom") {
            self.zoom = v;
        }
        match t.get_str("material") {
            Value::Str(s) => self.material = Some(s.to_string()),
            Value::User(u) => self.material = material_name(&u),
            _ => {}
        }
        match t.get_str("font") {
            Value::Str(s) => self.font = Some(s.to_string()),
            Value::User(u) if u.kind == "font" => {
                self.font = u.data.borrow().downcast_ref::<String>().cloned()
            }
            _ => {}
        }
        if let Some(v) = num("alignment") {
            self.alignment = v as i32;
        }
    }
}

/// A screen rectangle (x0, y0, x1, y1) in the root's units.
pub type Rect = [f32; 4];

/// Where an element's edges fall inside its parent's rectangle.
pub fn place(s: &State, parent: Rect) -> Rect {
    let axis = |lo_anchor: bool, hi_anchor: bool, lo: f32, hi: f32, p0: f32, p1: f32| -> (f32, f32) {
        let mid = (p0 + p1) * 0.5;
        match (lo_anchor, hi_anchor) {
            (true, true) => (p0 + lo, p1 + hi),
            (true, false) => (p0 + lo, p0 + hi),
            (false, true) => (p1 + lo, p1 + hi),
            (false, false) => (mid + lo, mid + hi),
        }
    };
    let (x0, x1) = axis(s.left_anchor, s.right_anchor, s.left, s.right, parent[0], parent[2]);
    let (y0, y1) = axis(s.top_anchor, s.bottom_anchor, s.top, s.bottom, parent[1], parent[3]);
    [x0, y0, x1, y1]
}

#[derive(Clone, Debug)]
struct Anim {
    name: String,
    from: State,
    to: State,
    start_ms: f64,
    duration_ms: f64,
    ease_in: bool,
    ease_out: bool,
}

/// The engine's part of one element.
pub struct Element {
    pub id: usize,
    pub parent: Option<Weak<UserData>>,
    pub children: Vec<Rc<UserData>>,
    pub priority: f32,
    /// The state now shown (animated toward `anim.to` while one runs).
    pub state: State,
    anim: Option<Anim>,
    /// The end state the setters write while an animation is open.
    pending: Option<State>,
    states: HashMap<String, TableRef>,
    pub use_stencil: bool,
    /// `setBlur(true)`: what is drawn under the element shows blurred (a
    /// front-end menu over another, CoD.Menu's updateBlur).
    pub blur: bool,
    /// bo2mp: drawn as the covered main menu behind Options / the Quit
    /// prompt (blurred and dimmed by the renderer).
    pub behind: bool,
    /// A list's gap between its children (`setSpacing`).
    pub spacing: f32,
    /// Where the last layout put it (`getRect`, the mouse's hit test).
    pub last_rect: Option<Rect>,
    /// Holds the menu focus (`setFocus`, `isInFocus`).
    pub focused: bool,
    /// A dashes bar's (count, lit, change) and units from one dash to the next
    /// (`setupDashes`).
    pub dashes: (i32, i32, i32),
    pub dash_pitch: f32,
    /// `setupTiles(n)`: a picture repeated across its width in tiles n units
    /// wide (BO2's dot grid backdrop: an 8-wide picture at 8); 0 = not tiled.
    pub tiles: f32,
    /// bo2mp: `setShaderVector(0, x, y, z, w)`: the globe's reveal (0: hidden,
    /// 1: wireframe, 2: its map in).
    pub shader: [f32; 4],
    pub kind: &'static str,
    /// The element's own field table (its metatable's `__newindex`).
    pub fields: TableRef,
    /// A streamed image (`setupUIStreamedImage`): how long it waits for its
    /// image (ms, 0 = for ever), and since when it has been waiting.
    stream_timeout: Option<f64>,
    stream_since: Option<f64>,
}

pub fn material_name(u: &UserData) -> Option<String> {
    if u.kind != "material" {
        return None;
    }
    u.data.borrow().downcast_ref::<String>().cloned()
}

/// The element behind a value, if it is one.
pub fn element(v: &Value) -> Option<Rc<UserData>> {
    match v {
        Value::User(u) if u.kind == "LUIElement" => Some(u.clone()),
        _ => None,
    }
}

pub(crate) fn with<R>(u: &UserData, f: impl FnOnce(&mut Element) -> R) -> R {
    let mut data = u.data.borrow_mut();
    let e = data.downcast_mut::<Element>().expect("LUIElement userdata holds an Element");
    f(e)
}

/// LUI's clock and the elements' animations (the engine drives `tick`).
pub struct Lui {
    pub now_ms: f64,
    next_id: usize,
    pub natives: TableRef,
    /// Elements whose animation ended since the last tick: the
    /// animation's name and whether a new one cut it short.
    finished: Vec<(Rc<UserData>, String, bool)>,
    pub roots: Vec<Rc<UserData>>,
    /// The roots' rectangle in their units (0,0 top left).
    pub root_rect: Rect,
}

/// bo2mp: menus covered by another menu are not drawn (BO2's front end
/// opens each screen over the last and marks the last `occludedBy`).
static HIDE_OCCLUDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// bo2mp: draw (false, the default) or hide (true) covered menus.
pub fn set_hide_occluded(on: bool) {
    HIDE_OCCLUDED.store(on, std::sync::atomic::Ordering::Relaxed);
}

thread_local! {
    /// How many covered main menus the layout is inside (see `walk`).
    static BEHIND: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// bo2mp: a menu another menu covers, when that menu blurs what is under it
/// (`setBlur`, set by CoD.Menu's `updateBlur` on every menu that covers one
/// in the front end, none named) and does not darken it itself: BO2 keeps
/// the covered menu drawn, and its blur and dim fall on it with the backdrop.
/// The renderer shows it blurred and dimmed (`Drawn::behind`).
fn is_behind_menu(u: &Rc<UserData>) -> bool {
    if !HIDE_OCCLUDED.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let by = with(u, |e| e.fields.borrow().get_str("occludedBy"));
    match &by {
        Value::User(o) if !std::ptr::eq(&**o, &**u) => with(o, |oe| !oe.fields.borrow().get_str("darkenElement").truthy() && oe.blur),
        _ => false,
    }
}

thread_local! {
    static LUI: RefCell<Option<Rc<RefCell<Lui>>>> = const { RefCell::new(None) };
}

fn lui() -> Rc<RefCell<Lui>> {
    LUI.with(|l| l.borrow().clone()).expect("LUI natives installed")
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Nil)
}

fn this(a: &[Value], f: &str) -> Res<Rc<UserData>> {
    element(&arg(a, 0)).ok_or_else(|| LuaError::new(format!("{f}: not called on an LUI element")))
}

fn ease(t: f64, ease_in: bool, ease_out: bool) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match (ease_in, ease_out) {
        (true, true) => t * t * (3.0 - 2.0 * t),
        (true, false) => t * t,
        (false, true) => 1.0 - (1.0 - t) * (1.0 - t),
        _ => t,
    }
}

/// The state the setters change now: the open animation's end, or the
/// shown state.
fn edit(e: &mut Element, f: impl FnOnce(&mut State)) {
    match e.pending.as_mut() {
        Some(p) => f(p),
        None => f(&mut e.state),
    }
}

/// A new element (`ConstructLUIElement`).
pub fn construct(vm: &mut Vm, kind: &'static str) -> Value {
    let l = lui();
    let (id, natives) = {
        let mut l = l.borrow_mut();
        l.next_id += 1;
        (l.next_id, l.natives.clone())
    };
    let fields = Table::new_ref();
    fields.borrow_mut().set_str("m_eventHandlers", Value::Table(Table::new_ref()));
    fields.borrow_mut().set_str("m_animationStates", Value::Table(Table::new_ref()));
    let meta = Table::new_ref();
    let f2 = fields.clone();
    let index = vm.native("LUIElement_index", move |vm, a| {
        let key = arg(&a, 1);
        let own = vm.index(&Value::Table(f2.clone()), &key)?;
        if !matches!(own, Value::Nil) {
            return Ok(vec![own]);
        }
        Ok(vec![natives.borrow().get(&key)])
    });
    meta.borrow_mut().set_str("__index", index);
    meta.borrow_mut().set_str("__newindex", Value::Table(fields.clone()));
    let el = Element {
        id,
        parent: None,
        children: Vec::new(),
        priority: 0.0,
        state: State::default(),
        anim: None,
        pending: None,
        states: HashMap::new(),
        use_stencil: false,
        blur: false,
        behind: false,
        spacing: 0.0,
        last_rect: None,
        focused: false,
        dashes: (0, 0, 0),
        dash_pitch: 8.0,
        tiles: 0.0,
        shader: [0.0; 4],
        kind,
        fields,
        stream_timeout: None,
        stream_since: None,
    };
    Value::User(Rc::new(UserData {
        kind: "LUIElement",
        data: RefCell::new(Box::new(el)),
        meta: RefCell::new(Some(meta)),
    }))
}

fn add_child(parent: &Rc<UserData>, child: &Rc<UserData>, at: Option<(usize, bool)>) {
    // Out of its old parent first.
    if let Some(old) = with(child, |e| e.parent.as_ref().and_then(Weak::upgrade)) {
        with(&old, |e| e.children.retain(|c| !Rc::ptr_eq(c, child)));
    }
    with(child, |e| e.parent = Some(Rc::downgrade(parent)));
    with(parent, |e| match at {
        Some((i, after)) => {
            let i = if after { i + 1 } else { i };
            e.children.insert(i.min(e.children.len()), child.clone());
        }
        None => e.children.push(child.clone()),
    });
}

/// bo2mp: a child the engine draws for an engine-made widget (the mini
/// playercard): a rectangle `[l, t, r, b]` from the parent's top left.
pub fn add_drawn_child(
    vm: &mut Vm,
    parent: &Rc<UserData>,
    rect: [f32; 4],
    rgba: [f32; 4],
    material: Option<&str>,
    text: Option<&str>,
    font: Option<String>,
) {
    let kind = if material.is_some() { "image" } else { "element" };
    // (Through the scripts' own constructor, so it answers events like any element.)
    let made = vm
        .index(&vm.global("LUI"), &Value::str("UIElement"))
        .and_then(|c| vm.index(&c, &Value::str("new")))
        .and_then(|f| vm.call(f, vec![]));
    let child = match made.ok().and_then(|r| r.into_iter().next()) {
        Some(Value::User(u)) => u,
        _ => match construct(vm, kind) {
            Value::User(u) => u,
            _ => return,
        },
    };
    with(&child, |e| e.kind = kind);
    with(&child, |e| {
        e.state.left_anchor = true;
        e.state.top_anchor = true;
        e.state.left = rect[0];
        e.state.top = rect[1];
        e.state.right = rect[2];
        e.state.bottom = rect[3];
        e.state.red = rgba[0];
        e.state.green = rgba[1];
        e.state.blue = rgba[2];
        e.state.alpha = rgba[3];
        e.state.material = material.map(str::to_owned);
        e.state.text = text.map(str::to_owned);
        e.state.font = font;
        e.state.alignment = 1;
    });
    add_child(parent, &child, None);
}

/// bo2mp: the font an element shows its text in now.
pub fn font_of(u: &UserData) -> Option<String> {
    with(u, |e| e.state.font.clone())
}

fn sibling_index(parent: &Rc<UserData>, sib: &Rc<UserData>) -> Option<usize> {
    with(parent, |e| e.children.iter().position(|c| Rc::ptr_eq(c, sib)))
}

/// Advance every running animation to `now_ms`; returns the elements whose
/// animation ended (their `transition_complete_<name>` goes to the
/// scripts).
/// Animations that ended: the element, the animation, whether a new one cut
/// it short, and how long after its end this tick saw it (`lateness`: a
/// 0 ms timer moves by it).
pub fn tick(now_ms: f64) -> Vec<(Rc<UserData>, String, bool, f64)> {
    let l = lui();
    l.borrow_mut().now_ms = now_ms;
    let roots = l.borrow().roots.clone();
    let mut done = Vec::new();
    let mut stack = roots;
    while let Some(u) = stack.pop() {
        let ended = with(&u, |e| {
            stack.extend(e.children.iter().cloned());
            let a = e.anim.as_ref()?;
            let t = if a.duration_ms <= 0.0 { 1.0 } else { (now_ms - a.start_ms) / a.duration_ms };
            let k = ease(t, a.ease_in, a.ease_out) as f32;
            e.state = a.from.mix(&a.to, if t >= 1.0 { 1.0 } else { k });
            if t >= 1.0 {
                let a = e.anim.take()?;
                e.state = a.to;
                Some((a.name, (now_ms - a.start_ms - a.duration_ms).max(0.0)))
            } else {
                None
            }
        });
        if let Some((name, late)) = ended {
            done.push((u.clone(), name, false, late));
        }
    }
    let mut l = l.borrow_mut();
    done.extend(l.finished.drain(..).map(|(u, n, i)| (u, n, i, 0.0)));
    done
}

/// Every element in drawing order (parents before children, children by
/// priority), with its rectangle and its alpha including its parents'.
pub fn layout(root: &Rc<UserData>, rect: Rect) -> Vec<(Rc<UserData>, Rect, f32)> {
    layout_clipped(root, rect).into_iter().map(|(u, r, a, _)| (u, r, a)).collect()
}

/// bo2mp: `layout` with each element's clip too: the rectangle of its
/// nearest ancestor that `setUseStencil`s (LUI's stencil clips the children
/// to that element's box; BO2's scorestreak and class tiles keep their
/// big pictures inside the tile this way), intersected with the clips above.
pub fn layout_clipped(root: &Rc<UserData>, rect: Rect) -> Vec<(Rc<UserData>, Rect, f32, Option<Rect>)> {
    let mut out = Vec::new();
    // `slot`: a list's child is placed along the list's axis by the list
    // (vertical?, start, end); across it by its own anchors. `xf`: where
    // the parents' scales put a point (k, x, y: p * k + (x, y)); each
    // element's own scale is about its centre, and its children's too.
    // bo2mp: the covered main menu stays drawn, blurred and dimmed by the
    // renderer (`Drawn::behind`), under Options and the Quit prompt: BO2
    // shows it so. Its children inherit the mark.
    fn walk(
        u: &Rc<UserData>,
        parent: Rect,
        alpha: f32,
        slot: Option<(bool, f32, f32)>,
        xf: (f32, f32, f32),
        mid: (f32, f32),
        clip: Option<Rect>,
        out: &mut Vec<(Rc<UserData>, Rect, f32, Option<Rect>)>,
    ) {
        let on = is_behind_menu(u);
        if on {
            BEHIND.with(|b| b.set(b.get() + 1));
        }
        walk_in(u, parent, alpha, slot, xf, mid, clip, out);
        if on {
            BEHIND.with(|b| b.set(b.get() - 1));
        }
    }
    fn walk_in(
        u: &Rc<UserData>,
        parent: Rect,
        alpha: f32,
        slot: Option<(bool, f32, f32)>,
        xf: (f32, f32, f32),
        mid: (f32, f32),
        clip: Option<Rect>,
        out: &mut Vec<(Rc<UserData>, Rect, f32, Option<Rect>)>,
    ) {
        let (r, a, mut kids, list, xf, shown, stencil) = with(u, |e| {
            let mut r = place(&e.state, parent);
            match slot {
                Some((true, y0, y1)) => (r[1], r[3]) = (y0, y1),
                Some((false, x0, x1)) => (r[0], r[2]) = (x0, x1),
                None => {}
            }
            let s = e.state.scale;
            let (cx, cy) = ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
            let xf = (xf.0 * s, xf.0 * cx * (1.0 - s) + xf.1, xf.0 * cy * (1.0 - s) + xf.2);
            // `zoom` (toward the camera) scales everything about the screen's
            // centre, after the parents' and its own scale.
            let xf = if e.state.zoom == 0.0 {
                xf
            } else {
                let z = 1.0 / (1.0 - e.state.zoom / 1280.0);
                (xf.0 * z, z * (xf.1 - mid.0) + mid.0, z * (xf.2 - mid.1) + mid.1)
            };
            let shown = [r[0] * xf.0 + xf.1, r[1] * xf.0 + xf.2, r[2] * xf.0 + xf.1, r[3] * xf.0 + xf.2];
            let a = alpha * e.state.alpha * e.state.alpha_multiplier;
            // bo2mp: a menu another menu covers (`occludedBy`, set by
            // CoD.Menu's occlusion_change) is not drawn (`set_hide_occluded`).
            // An in-game popup (End Game?) keeps what it covers drawn: the
            // popup darkens it itself (`darkenElement`).
            let by = e.fields.borrow().get_str("occludedBy");
            let is_hud = e.fields.borrow().get_str("menuName").as_str() == Some("HUD");
            let by_dimmer = match &by {
                _ if is_hud => false,
                Value::User(o) if !std::ptr::eq(&**o, &**u) => {
                    with(o, |oe| oe.fields.borrow().get_str("darkenElement").truthy())
                }
                _ => false,
            };
            let behind = BEHIND.with(|b| b.get()) > 0;
            e.behind = behind;
            let a = if HIDE_OCCLUDED.load(std::sync::atomic::Ordering::Relaxed)
                && by.truthy()
                && !by_dimmer
                && !behind
            {
                0.0
            } else {
                a
            };
            e.last_rect = Some(shown);
            let list = match e.kind {
                "vlist" => Some((true, e.spacing, e.state.alignment)),
                "hlist" => Some((false, e.spacing, e.state.alignment)),
                _ => None,
            };
            (r, a, e.children.clone(), list, xf, shown, e.use_stencil)
        });
        out.push((u.clone(), shown, a, clip));
        let clip = if stencil {
            let b = [shown[0].min(shown[2]), shown[1].min(shown[3]), shown[0].max(shown[2]), shown[1].max(shown[3])];
            Some(match clip {
                Some(c) => [b[0].max(c[0]), b[1].max(c[1]), b[2].min(c[2]), b[3].min(c[3])],
                None => b,
            })
        } else {
            clip
        };
        // bo2mp: a menu's `darkenElement` (the popup's full-screen black dim,
        // added by setOccludedMenu after the popup's panel, at the panel's
        // own priority -100) is drawn first among equals: BO2 shows the End
        // Game panel lit over its dim, not dimmed by it.
        let dim = with(u, |e| e.fields.borrow().get_str("darkenElement"));
        let is_dim = |k: &Rc<UserData>| matches!(&dim, Value::User(o) if std::ptr::eq(&**o, &**k));
        kids.sort_by(|x, y| {
            let px = with(x, |e| e.priority);
            let py = with(y, |e| e.priority);
            px.partial_cmp(&py).unwrap_or(std::cmp::Ordering::Equal).then_with(|| is_dim(y).cmp(&is_dim(x)))
        });
        let Some((vertical, spacing, align)) = list else {
            for k in &kids {
                walk(k, r, a, None, xf, mid, clip, out);
            }
            return;
        };
        // A list: its children one after another along its axis, each its
        // own size, `spacing` apart; the run starts at the list's start,
        // middle or end (LUI.Alignment top/left, middle/centre,
        // bottom/right).
        let sizes: Vec<f32> = kids
            .iter()
            .map(|k| {
                with(k, |e| {
                    if vertical {
                        (e.state.bottom - e.state.top).abs()
                    } else {
                        (e.state.right - e.state.left).abs()
                    }
                })
            })
            .collect();
        // bo2mp: a zero-size child of a vertical list (Options' two empty
        // spacer elements) takes no size and no spacing: BO2's SETTINGS
        // starts at the list's top.
        let skip = |sz: f32| vertical && sz == 0.0;
        let count = sizes.iter().filter(|s| !skip(**s)).count();
        let total = sizes.iter().sum::<f32>() + spacing * count.saturating_sub(1) as f32;
        let (lo, hi) = if vertical { (r[1], r[3]) } else { (r[0], r[2]) };
        let mut at = match align {
            2 | 5 => (lo + hi) * 0.5 - total * 0.5,
            3 | 6 => hi - total,
            _ => lo,
        };
        // A right-aligned row runs from its right edge: its first child is
        // the rightmost (BO2's right button prompts - "R Remove  C
        // Personalize Weapon", C added first - and its tab headers).
        let mut order: Vec<(Rc<UserData>, f32)> = kids.iter().cloned().zip(sizes).collect();
        if !vertical && align == 3 {
            order.reverse();
        }
        for (k, size) in order.iter().map(|(k, s)| (k, *s)) {
            walk(k, r, a, Some((vertical, at, at + size)), xf, mid, clip, out);
            if !skip(size) {
                at += size + spacing;
            }
        }
    }
    walk(root, rect, 1.0, None, (1.0, 0.0, 0.0), ((rect[0] + rect[2]) * 0.5, (rect[1] + rect[3]) * 0.5), None, &mut out);
    out
}

fn reg(vm: &mut Vm, t: &TableRef, name: &'static str, f: impl Fn(&mut Vm, Vec<Value>) -> Res<Vec<Value>> + 'static) {
    // T6LUA_TRACE=1 prints every element call (debugging aid).
    let trace = std::env::var_os("T6LUA_TRACE").is_some();
    let v = vm.native(name, move |vm, a| {
        if trace {
            let id = a.first().and_then(element).map_or_else(|| "-".to_owned(), |u| with(&u, |e| e.id.to_string()));
            let rest: Vec<String> = a.iter().skip(1).map(|v| format!("{v:?}")).collect();
            println!("  call #{id} {name}({})", rest.join(", "));
        }
        f(vm, a)
    });
    t.borrow_mut().set_str(name, v);
}

/// bo2mp: one more element native (`self:<name>(...)`) beside `install`'s,
/// for a game's own widgets. Call after `install`, before elements exist.
pub fn add_native(vm: &mut Vm, name: &'static str, f: impl Fn(&mut Vm, Vec<Value>) -> Res<Vec<Value>> + 'static) {
    let t = lui().borrow().natives.clone();
    reg(vm, &t, name, f);
}

/// bo2mp: the element natives' table.
pub fn natives() -> TableRef {
    lui().borrow().natives.clone()
}

/// bo2mp: an element native that names the element's kind (an
/// engine-drawn widget not drawn yet), as `install`'s `setupUIImage` ones.
pub fn add_kind_native(vm: &mut Vm, name: &'static str, kind: &'static str) {
    add_native(vm, name, move |_, a| {
        let u = this(&a, name)?;
        with(&u, |e| e.kind = kind);
        Ok(vec![])
    });
}

/// Install `ConstructLUIElement`, `RegisterMaterial` and the element
/// natives into the VM.
#[allow(clippy::too_many_lines)]
pub fn install(vm: &mut Vm) {
    let natives = Table::new_ref();
    let l = Rc::new(RefCell::new(Lui {
        now_ms: 0.0,
        next_id: 0,
        natives: natives.clone(),
        finished: Vec::new(),
        roots: Vec::new(),
        root_rect: [0.0, 0.0, 1280.0, 720.0],
    }));
    LUI.with(|slot| *slot.borrow_mut() = Some(l));
    let n = &natives;

    reg(vm, n, "setLeftRight", |_, a| {
        let u = this(&a, "setLeftRight")?;
        let (la, ra) = (arg(&a, 1).truthy(), arg(&a, 2).truthy());
        let (l, r) = (arg(&a, 3).as_num().unwrap_or(0.0), arg(&a, 4).as_num().unwrap_or(0.0));
        with(&u, |e| {
            edit(e, |s| {
                s.left_anchor = la;
                s.right_anchor = ra;
                s.left = l;
                s.right = r;
            });
        });
        Ok(vec![])
    });
    reg(vm, n, "setTopBottom", |_, a| {
        let u = this(&a, "setTopBottom")?;
        let (ta, ba) = (arg(&a, 1).truthy(), arg(&a, 2).truthy());
        let (t, b) = (arg(&a, 3).as_num().unwrap_or(0.0), arg(&a, 4).as_num().unwrap_or(0.0));
        with(&u, |e| {
            edit(e, |s| {
                s.top_anchor = ta;
                s.bottom_anchor = ba;
                s.top = t;
                s.bottom = b;
            });
        });
        Ok(vec![])
    });
    reg(vm, n, "setAlpha", |_, a| {
        let u = this(&a, "setAlpha")?;
        let v = arg(&a, 1).as_num().unwrap_or(1.0);
        with(&u, |e| edit(e, |s| s.alpha = v));
        Ok(vec![])
    });
    reg(vm, n, "setRGB", |_, a| {
        let u = this(&a, "setRGB")?;
        let c = [1, 2, 3].map(|i| arg(&a, i).as_num().unwrap_or(1.0));
        with(&u, |e| {
            edit(e, |s| {
                s.red = c[0];
                s.green = c[1];
                s.blue = c[2];
            });
        });
        Ok(vec![])
    });
    reg(vm, n, "setScale", |_, a| {
        let u = this(&a, "setScale")?;
        let v = arg(&a, 1).as_num().unwrap_or(1.0);
        with(&u, |e| edit(e, |s| s.scale = v));
        Ok(vec![])
    });
    for (name, axis) in [("setXRot", 0usize), ("setYRot", 1), ("setZRot", 2)] {
        reg(vm, n, name, move |_, a| {
            let u = this(&a, name)?;
            let v = arg(&a, 1).as_num().unwrap_or(0.0);
            with(&u, |e| {
                edit(e, |s| match axis {
                    0 => s.x_rot = v,
                    1 => s.y_rot = v,
                    _ => s.z_rot = v,
                });
            });
            Ok(vec![])
        });
    }
    reg(vm, n, "setImage", |_, a| {
        let u = this(&a, "setImage")?;
        let m = match arg(&a, 1) {
            Value::User(m) => material_name(&m),
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        };
        let now = lui().borrow().now_ms;
        with(&u, |e| {
            // A streamed image streams each new image in.
            if e.stream_timeout.is_some() {
                e.stream_since = Some(now);
            }
            edit(e, |s| s.material = m);
        });
        Ok(vec![])
    });
    for name in ["setText", "setTextInC"] {
        reg(vm, n, name, move |vm, a| {
            let u = this(&a, name)?;
            let t = match arg(&a, 1) {
                Value::Nil => None,
                v => Some(vm.tostring(&v)?),
            };
            with(&u, |e| edit(e, |s| s.text = t));
            Ok(vec![])
        });
    }
    reg(vm, n, "setFont", |_, a| {
        let u = this(&a, "setFont")?;
        let f = match arg(&a, 1) {
            Value::Str(s) => Some(s.to_string()),
            Value::User(f) => f.data.borrow().downcast_ref::<String>().cloned(),
            _ => None,
        };
        with(&u, |e| edit(e, |s| s.font = f));
        Ok(vec![])
    });
    reg(vm, n, "setAlignment", |_, a| {
        let u = this(&a, "setAlignment")?;
        // bo2mp: setAlignment(nil) (the Barracks League block's text
        // helper passes the missing LUI.UIElement.Left) reads left in BO2.
        let v = match arg(&a, 1) {
            Value::Nil => 1,
            x => x.as_num().unwrap_or(0.0) as i32,
        };
        with(&u, |e| edit(e, |s| s.alignment = v));
        Ok(vec![])
    });
    reg(vm, n, "setSpacing", |_, a| {
        let u = this(&a, "setSpacing")?;
        let v = arg(&a, 1).as_num().unwrap_or(0.0);
        with(&u, |e| e.spacing = v);
        Ok(vec![])
    });
    reg(vm, n, "setPriority", |_, a| {
        let u = this(&a, "setPriority")?;
        let v = arg(&a, 1).as_num().unwrap_or(0.0);
        with(&u, |e| e.priority = v);
        Ok(vec![])
    });
    reg(vm, n, "setUseStencil", |_, a| {
        let u = this(&a, "setUseStencil")?;
        let v = arg(&a, 1).truthy();
        with(&u, |e| e.use_stencil = v);
        Ok(vec![])
    });
    for name in [
        "setLayoutCached",
        "setUseGameTime",
        "updateElementLayout",
        "layoutChildren",
        "setRoot",
    ] {
        reg(vm, n, name, |_, _| Ok(vec![]));
    }
    reg(vm, n, "setShaderVector", |_, a| {
        let u = this(&a, "setShaderVector")?;
        if arg(&a, 1).as_num().unwrap_or(-1.0) == 0.0 {
            let v = |i: usize| arg(&a, i).as_num().unwrap_or(0.0) as f32;
            with(&u, |e| e.shader = [v(2), v(3), v(4), v(5)]);
        }
        Ok(vec![])
    });
    // setupDashes(count, filled, ...): a slider's bar, `count` dashes of
    // which `filled` are lit (the engine draws it).
    reg(vm, n, "setupDashes", |_, a| {
        let u = this(&a, "setupDashes")?;
        let count = arg(&a, 1).as_num().unwrap_or(0.0) as i32;
        let filled = arg(&a, 2).as_num().unwrap_or(0.0) as i32;
        // (count, lit, change, pitch): 20 dashes 8 units apart make the
        // slider's 160-unit bar. The third is an attachment's change: that
        // many dashes better (positive) or worse (negative) than `filled`,
        // drawn in the green or red pip.
        let change = arg(&a, 3).as_num().unwrap_or(0.0) as i32;
        let pitch = arg(&a, 4).as_num().unwrap_or(8.0);
        with(&u, |e| {
            e.kind = "dashes";
            e.dashes = (count.max(0), filled.clamp(0, count.max(0)), change);
            e.dash_pitch = pitch.max(1.0);
        });
        Ok(vec![])
    });
    // setupTiles(n): the picture repeats across its width in tiles n units wide.
    reg(vm, n, "setupTiles", |_, a| {
        let u = this(&a, "setupTiles")?;
        let t = arg(&a, 1).as_num().unwrap_or(0.0);
        with(&u, |e| {
            e.kind = "image";
            e.tiles = t.max(0.0);
        });
        Ok(vec![])
    });
    // Focus: the scripts move it (gain_focus / lose_focus) and ask it.
    reg(vm, n, "setFocus", |_, a| {
        let u = this(&a, "setFocus")?;
        let on = arg(&a, 1).truthy();
        with(&u, |e| e.focused = on);
        Ok(vec![])
    });
    reg(vm, n, "isInFocus", |_, a| {
        let u = this(&a, "isInFocus")?;
        Ok(vec![Value::Bool(with(&u, |e| e.focused))])
    });
    reg(vm, n, "registerAnimationState", |_, a| {
        let u = this(&a, "registerAnimationState")?;
        let name = arg(&a, 1).as_str().unwrap_or("").to_owned();
        if let Value::Table(t) = arg(&a, 2) {
            // The scripts also read them: `self.m_animationStates.<name>`.
            with(&u, |e| {
                if let Value::Table(m) = e.fields.borrow().get_str("m_animationStates") {
                    m.borrow_mut().set_str(&name, Value::Table(t.clone()));
                }
                e.states.insert(name, t)
            });
        }
        Ok(vec![])
    });
    reg(vm, n, "beginAnimation", |_, a| {
        let u = this(&a, "beginAnimation")?;
        let name = arg(&a, 1).as_str().unwrap_or("").to_owned();
        let ms = f64::from(arg(&a, 2).as_num().unwrap_or(0.0));
        let (ei, eo) = (arg(&a, 3).truthy(), arg(&a, 4).truthy());
        let now = lui().borrow().now_ms;
        let interrupted = with(&u, |e| {
            let old = e.anim.take().map(|a| a.name);
            let from = e.state.clone();
            e.pending = Some(from.clone());
            e.anim = Some(Anim {
                name,
                from: from.clone(),
                to: from,
                start_ms: now,
                duration_ms: ms,
                ease_in: ei,
                ease_out: eo,
            });
            old
        });
        if let Some(old) = interrupted {
            lui().borrow_mut().finished.push((u, old, true));
        }
        Ok(vec![])
    });
    // How far the running animation is (0 to 1); 1 when none runs.
    reg(vm, n, "getAnimationFraction", |_, a| {
        let u = this(&a, "getAnimationFraction")?;
        let now = lui().borrow().now_ms;
        let f = with(&u, |e| match &e.anim {
            Some(a) if a.duration_ms > 0.0 => ((now - a.start_ms) / a.duration_ms).clamp(0.0, 1.0),
            _ => 1.0,
        });
        Ok(vec![Value::Num(f as _)])
    });
    reg(vm, n, "animateToState", |_, a| {
        let u = this(&a, "animateToState")?;
        let name = arg(&a, 1).as_str().unwrap_or("").to_owned();
        let ms = f64::from(arg(&a, 2).as_num().unwrap_or(0.0));
        let (ei, eo) = (arg(&a, 3).truthy(), arg(&a, 4).truthy());
        let now = lui().borrow().now_ms;
        with(&u, |e| {
            let Some(t) = e.states.get(&name).cloned() else { return };
            let mut to = e.state.clone();
            to.apply_table(&t.borrow());
            // A list's state names its spacing too (CoD.Menu's button
            // prompt bars: `spacing = 10`).
            if let Some(sp) = t.borrow().get_str("spacing").as_num() {
                e.spacing = sp;
            }
            e.pending = None;
            if ms <= 0.0 {
                e.state = to;
                e.anim = None;
            } else {
                e.anim = Some(Anim {
                    name: name.clone(),
                    from: e.state.clone(),
                    to,
                    start_ms: now,
                    duration_ms: ms,
                    ease_in: ei,
                    ease_out: eo,
                });
            }
        });
        Ok(vec![])
    });
    reg(vm, n, "completeAnimation", |_, a| {
        let u = this(&a, "completeAnimation")?;
        let name = with(&u, |e| {
            e.pending = None;
            let a = e.anim.take()?;
            e.state = a.to;
            Some(a.name)
        });
        if let Some(name) = name {
            lui().borrow_mut().finished.push((u, name, false));
        }
        Ok(vec![])
    });
    // The setters after beginAnimation write `pending`; it becomes the
    // animation's end when the next frame starts it running.
    reg(vm, n, "addElementToC", |_, a| {
        let (p, c) = (this(&a, "addElementToC")?, element(&arg(&a, 1)));
        if let Some(c) = c {
            add_child(&p, &c, None);
        }
        Ok(vec![])
    });
    for (name, after) in [("addElementBeforeInC", false), ("addElementAfterInC", true)] {
        reg(vm, n, name, move |_, a| {
            // self:addElementBeforeInC(sibling): self goes before sibling.
            let (me, sib) = (this(&a, name)?, element(&arg(&a, 1)));
            let Some(sib) = sib else { return Ok(vec![]) };
            let Some(parent) = with(&sib, |e| e.parent.as_ref().and_then(Weak::upgrade)) else {
                return Ok(vec![]);
            };
            let i = sibling_index(&parent, &sib).unwrap_or(0);
            add_child(&parent, &me, Some((i, after)));
            Ok(vec![])
        });
    }
    reg(vm, n, "removeFromParentInC", |_, a| {
        let u = this(&a, "removeFromParentInC")?;
        if let Some(p) = with(&u, |e| e.parent.take().and_then(|w| w.upgrade())) {
            with(&p, |e| e.children.retain(|c| !Rc::ptr_eq(c, &u)));
        }
        Ok(vec![])
    });
    // removeElement(child): the child out of this element.
    reg(vm, n, "removeElement", |_, a| {
        let p = this(&a, "removeElement")?;
        if let Some(c) = element(&arg(&a, 1)) {
            let mine = with(&c, |e| e.parent.as_ref().and_then(Weak::upgrade)).is_some_and(|q| Rc::ptr_eq(&q, &p));
            if mine {
                with(&c, |e| e.parent = None);
                with(&p, |e| e.children.retain(|x| !Rc::ptr_eq(x, &c)));
            }
        }
        Ok(vec![])
    });
    reg(vm, n, "removeAllChildren", |_, a| {
        let u = this(&a, "removeAllChildren")?;
        let kids = with(&u, |e| std::mem::take(&mut e.children));
        for k in kids {
            with(&k, |e| e.parent = None);
        }
        Ok(vec![])
    });
    reg(vm, n, "getParent", |_, a| {
        let u = this(&a, "getParent")?;
        Ok(vec![with(&u, |e| e.parent.as_ref().and_then(Weak::upgrade)).map_or(Value::Nil, Value::User)])
    });
    reg(vm, n, "getFirstChild", |_, a| {
        let u = this(&a, "getFirstChild")?;
        Ok(vec![with(&u, |e| e.children.first().cloned()).map_or(Value::Nil, Value::User)])
    });
    reg(vm, n, "getLastChild", |_, a| {
        let u = this(&a, "getLastChild")?;
        Ok(vec![with(&u, |e| e.children.last().cloned()).map_or(Value::Nil, Value::User)])
    });
    reg(vm, n, "getNumChildren", |_, a| {
        let u = this(&a, "getNumChildren")?;
        Ok(vec![Value::Num(with(&u, |e| e.children.len()) as f32)])
    });
    reg(vm, n, "getNextSibling", |_, a| {
        let u = this(&a, "getNextSibling")?;
        let Some(p) = with(&u, |e| e.parent.as_ref().and_then(Weak::upgrade)) else {
            return Ok(vec![Value::Nil]);
        };
        let i = sibling_index(&p, &u);
        Ok(vec![
            i.and_then(|i| with(&p, |e| e.children.get(i + 1).cloned()))
                .map_or(Value::Nil, Value::User),
        ])
    });
    reg(vm, n, "getPreviousSibling", |_, a| {
        let u = this(&a, "getPreviousSibling")?;
        let Some(p) = with(&u, |e| e.parent.as_ref().and_then(Weak::upgrade)) else {
            return Ok(vec![Value::Nil]);
        };
        let i = sibling_index(&p, &u);
        Ok(vec![
            i.filter(|i| *i > 0)
                .and_then(|i| with(&p, |e| e.children.get(i - 1).cloned()))
                .map_or(Value::Nil, Value::User),
        ])
    });
    reg(vm, n, "getRect", |_, a| {
        // The rectangle in the root's units (left, top, right, bottom).
        let u = this(&a, "getRect")?;
        // As last drawn (lists place their children), else from the anchors.
        if let Some(r) = with(&u, |e| e.last_rect) {
            return Ok(r.iter().map(|v| Value::Num(*v)).collect());
        }
        let mut chain = vec![u.clone()];
        let mut at = u;
        while let Some(p) = with(&at, |e| e.parent.as_ref().and_then(Weak::upgrade)) {
            chain.push(p.clone());
            at = p;
        }
        let mut rect: Rect = lui().borrow().root_rect;
        for e in chain.iter().rev() {
            rect = with(e, |e| place(&e.state, rect));
        }
        Ok(rect.iter().map(|v| Value::Num(*v)).collect())
    });
    // setupUIImage / setupUIText / ...: the element's kind.
    for (name, kind) in [
        ("setupUIElement", "element"),
        ("setupUIImage", "image"),
        ("setupUIText", "text"),
        ("setupUITextUncached", "text"),
        ("setupUITightText", "text"),
        ("setupUIHorizontalList", "hlist"),
        ("setupUIVerticalList", "vlist"),
        ("setupGameTimer", "timer"),
        ("setupGameTimerZombie", "timer"),
        // Engine-drawn widgets not drawn yet: their kind names them.
        ("setupLoadingBar", "loadingbar"),
        ("setupLoadingStatusText", "text"),
        ("setupSafeAreaBoundary", "element"),
        ("setupEntityContainer", "entity"),
        ("setupGameMessages", "messages"),
        ("setupObjectiveProgress", "progress"),
        ("setupHUDShaker", "element"),
        ("setupVoipImage", "voip"),
        ("setupVoiceMeter", "voip"),
        ("setupCinematicSubtitles", "subtitles"),
        ("setupEdgePointer", "pointer"),
        ("setupHorizontalCompass", "compass"),
        ("setupPlayerHealthEKG", "element"),
        ("setupVisorImage", "image"),
        ("setupImageViewer", "image"),
        ("setupPlayerEmblemServer", "image"),
        ("setupPlayerEmblemByXUID", "image"),
        ("setupLeagueEmblem", "image"),
        ("setupEmblem", "element"),
        ("setupEmblemBackgrounds", "element"),
        ("setupEmblemHiddenLayer", "element"),
        ("setupEmblemSelector", "element"),
        ("setupEmblemIcons", "element"),
        ("setupEmblemCopyWidget", "element"),
    ] {
        reg(vm, n, name, move |_, a| {
            let u = this(&a, name)?;
            with(&u, |e| {
                e.kind = kind;
                // A player's emblem is the engine's picture; with none to
                // draw it is empty (never the white square of no material).
                // The Solo team's emblem (drawn by the host: no material
                // holds the engine's picture of a team's emblem).
                if name == "setupLeagueEmblem" {
                    e.state.material = Some("\u{1}bo2mp league emblem solo".to_owned());
                }
                if name.starts_with("setupPlayerEmblem") && e.state.material.as_deref().is_none_or(str::is_empty) {
                    // (A player with none of his own shows his rank's
                    // default: the PFC chevron in the rank green.)
                    e.state.material = Some("em_rank_pvt_full".to_owned());
                    (e.state.red, e.state.green, e.state.blue) = (0.431, 0.584, 0.055);
                }
            });
            Ok(vec![])
        });
    }
    // setupVoipImage(clientNum): the engine's speaker beside a player's
    // name; no one talks here: its not-talking speaker (`nottalkingicon`).
    reg(vm, n, "setupVoipImage", |_, a| {
        let u = this(&a, "setupVoipImage")?;
        with(&u, |e| {
            e.kind = "image";
            e.state.material = Some("nottalkingicon".to_owned());
        });
        Ok(vec![])
    });

    // setupUIStreamedImage(timeout): an image the engine streams in; it
    // tells the element `streamed_image_ready` (or `_timed_out`), see
    // `stream_events`.
    reg(vm, n, "setupUIStreamedImage", |_, a| {
        let u = this(&a, "setupUIStreamedImage")?;
        let timeout = arg(&a, 1).as_num().map_or(0.0, f64::from);
        let now = lui().borrow().now_ms;
        with(&u, |e| {
            e.kind = "image";
            e.stream_timeout = Some(timeout);
            e.stream_since = Some(now);
        });
        Ok(vec![])
    });

    reg(vm, n, "setBlur", |_, a| {
        let u = this(&a, "setBlur")?;
        let v = arg(&a, 1).truthy();
        with(&u, |e| e.blur = v);
        Ok(vec![])
    });
    // Engine settings on widgets not drawn yet (no effect here).
    for name in [
        "setZoom",
        "setTileVertically",
        "setEntityContainerClamp",
        "setEntityContainerFadeWhenTargeted",
        "setEntityContainerStopUpdating",
        "setOwnerControllerIndex",
        "setUI3DWindow",
        "addCompass",
        "addCrosshairDistance",
    ] {
        reg(vm, n, name, |_, _| Ok(vec![]));
    }

    let construct_fn = vm.native("ConstructLUIElement", |vm, _| Ok(vec![construct(vm, "element")]));
    vm.set_global("ConstructLUIElement", construct_fn);
    let register_material = vm.native("RegisterMaterial", |_, a| {
        let name = arg(&a, 0).as_str().unwrap_or("").to_owned();
        Ok(vec![Value::User(Rc::new(UserData {
            kind: "material",
            data: RefCell::new(Box::new(name)),
            meta: RefCell::new(None),
        }))])
    });
    vm.set_global("RegisterMaterial", register_material);
    let register_font = vm.native("RegisterFont", |_, a| {
        let name = arg(&a, 0).as_str().unwrap_or("").to_owned();
        Ok(vec![Value::User(Rc::new(UserData {
            kind: "font",
            data: RefCell::new(Box::new(name)),
            meta: RefCell::new(None),
        }))])
    });
    vm.set_global("RegisterFont", register_font);
    let pairs = vm.global("pairs");
    vm.set_global("hpairs", pairs);
    // ProjectRootCoordinate(rootName, x, y): the engine's mouse position in a
    // root's units (the host sends it in root units already).
    let project = vm.native("ProjectRootCoordinate", |_, a| Ok(vec![arg(&a, 1), arg(&a, 2)]));
    vm.set_global("ProjectRootCoordinate", project);
}

/// Streamed images whose wait ended this frame, with the event the engine
/// sends them: `streamed_image_ready` once the image is set (the images
/// here are read straight from the game's files, so they are in as soon
/// as they are named), `streamed_image_timed_out` when none came in time.
pub fn stream_events(now_ms: f64) -> Vec<(Rc<UserData>, &'static str)> {
    let l = lui();
    let roots = l.borrow().roots.clone();
    let mut out = Vec::new();
    let mut stack = roots;
    while let Some(u) = stack.pop() {
        let ev = with(&u, |e| {
            stack.extend(e.children.iter().cloned());
            let since = e.stream_since?;
            let named = e.pending.as_ref().map_or(&e.state, |p| p).material.is_some();
            let timeout = e.stream_timeout.unwrap_or(0.0);
            let ev = if named {
                "streamed_image_ready"
            } else if timeout > 0.0 && now_ms - since >= timeout {
                "streamed_image_timed_out"
            } else {
                return None;
            };
            e.stream_since = None;
            Some(ev)
        });
        if let Some(ev) = ev {
            out.push((u.clone(), ev));
        }
    }
    out
}

/// Start the animations whose end the setters have written since their
/// `beginAnimation` (called once per frame before `tick`).
pub fn commit_pending() {
    let l = lui();
    let roots = l.borrow().roots.clone();
    let mut stack = roots;
    while let Some(u) = stack.pop() {
        with(&u, |e| {
            stack.extend(e.children.iter().cloned());
            if let Some(p) = e.pending.take()
                && let Some(a) = e.anim.as_mut()
            {
                a.to = p;
            }
        });
    }
}

/// Make an element a root (its tree is laid out and animated).
pub fn add_root(u: Rc<UserData>) {
    lui().borrow_mut().roots.push(u);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_place_edges() {
        let parent = [0.0, 0.0, 100.0, 50.0];
        let mut s = State { left_anchor: true, right_anchor: true, left: 10.0, right: -10.0, ..State::default() };
        assert_eq!(place(&s, parent)[0..1], [10.0]);
        assert_eq!(place(&s, parent)[2], 90.0);
        s.right_anchor = false;
        s.right = 30.0;
        assert_eq!(place(&s, parent)[2], 30.0);
        s.left_anchor = false;
        s.left = -5.0;
        s.right = 5.0;
        let r = place(&s, parent);
        assert_eq!((r[0], r[2]), (45.0, 55.0));
    }

    /// A vertical list stacks its children by their own heights, `spacing`
    /// apart, from its top (or centred, LUI.Alignment.Middle = 5).
    #[test]
    fn vertical_list_stacks_children() {
        let mut vm = Vm::new();
        install(&mut vm);
        let list = element(&construct(&mut vm, "vlist")).unwrap();
        with(&list, |e| {
            e.spacing = 2.0;
            e.state.left_anchor = true;
            e.state.right_anchor = true;
            e.state.top_anchor = true;
            e.state.bottom_anchor = true;
        });
        for h in [30.0, 30.0] {
            let c = element(&construct(&mut vm, "element")).unwrap();
            with(&c, |e| {
                e.state.left_anchor = true;
                e.state.right_anchor = true;
                e.state.top_anchor = true;
                e.state.bottom = h;
            });
            add_child(&list, &c, None);
        }
        let laid = layout(&list, [0.0, 0.0, 100.0, 100.0]);
        let tops: Vec<(f32, f32)> = laid.iter().skip(1).map(|(_, r, _, _)| (r[1], r[3])).collect();
        assert_eq!(tops, vec![(0.0, 30.0), (32.0, 62.0)]);
        with(&list, |e| e.state.alignment = 5);
        let laid = layout(&list, [0.0, 0.0, 100.0, 100.0]);
        assert_eq!(laid[1].1[1], 19.0);
    }
}

/// The names of the element natives installed (after `install`).
pub fn native_names() -> Vec<String> {
    let l = lui();
    let natives = l.borrow().natives.clone();
    let t = natives.borrow();
    let mut out = Vec::new();
    let mut k = Value::Nil;
    while let Some((key, _)) = t.next(&k) {
        out.push(key.to_string());
        k = key;
    }
    out
}

/// LUI's clock (the last `tick`'s time).
pub fn now_ms() -> f64 {
    lui().borrow().now_ms
}

/// Set the roots' rectangle (the host, on a resize).
pub fn set_root_rect(r: Rect) {
    lui().borrow_mut().root_rect = r;
}

/// The element tree under `u` as indented lines: id, kind, focus, the
/// fields that gate input (`m_inputDisabled`, `m_ownerController`) and
/// the element's `id` field (debugging aid).
pub fn tree(u: &Rc<UserData>, depth: usize, out: &mut Vec<String>) {
    let (line, kids) = with(u, |e| {
        let f = e.fields.borrow();
        let name = f.get_str("id");
        let off = f.get_str("m_inputDisabled");
        let owner = f.get_str("m_ownerController");
        (
            format!(
                "{:indent$}#{} {}{}{}{} {}",
                "",
                e.id,
                e.kind,
                if e.focused { " FOCUS" } else { "" },
                if off.truthy() { " input-off" } else { "" },
                if matches!(owner, Value::Nil) { String::new() } else { format!(" owner={owner}") },
                if matches!(name, Value::Nil) { String::new() } else { name.to_string() },
                indent = depth * 2
            ),
            e.children.clone(),
        )
    });
    out.push(line);
    for k in &kids {
        tree(k, depth + 1, out);
    }
}

/// The element with this id under `u` (debugging aid).
pub fn find(u: &Rc<UserData>, id: usize) -> Option<Rc<UserData>> {
    let (me, kids) = with(u, |e| (e.id == id, e.children.clone()));
    if me {
        return Some(u.clone());
    }
    kids.iter().find_map(|k| find(k, id))
}

/// The `id` fields of an element's children (a menu's is `Menu.<name>`).
pub fn child_ids(u: &Rc<UserData>) -> Vec<String> {
    let kids = with(u, |e| e.children.clone());
    kids.iter()
        .filter_map(|k| with(k, |e| e.fields.borrow().get_str("id").as_str().map(str::to_owned)))
        .collect()
}

/// An element's id.
pub fn element_id(u: &Rc<UserData>) -> usize {
    with(u, |e| e.id)
}
