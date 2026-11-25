use crate::app::{display_metric, Editor, Item, Tool};
use crate::wiring::{self, Target};
use cirtam_core::{
    circuit::{Circuit, Component, Kind, Point, Wire},
    diagnostics::{Location, Severity},
};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct Palette {
    background: Color32,
    grid: Color32,
    ink: Color32,
    wire: Color32,
    selected: Color32,
    enclosure: Color32,
}

impl Palette {
    fn for_theme(dark: bool) -> Self {
        if dark {
            Self {
                background: Color32::from_rgb(19, 25, 33),
                grid: Color32::from_rgb(55, 67, 81),
                ink: Color32::from_rgb(208, 222, 234),
                wire: Color32::from_rgb(106, 196, 177),
                selected: Color32::from_rgb(255, 192, 104),
                enclosure: Color32::LIGHT_BLUE,
            }
        } else {
            Self {
                background: Color32::from_rgb(248, 250, 252),
                grid: Color32::from_rgb(175, 187, 200),
                ink: Color32::from_rgb(35, 49, 64),
                wire: Color32::from_rgb(0, 105, 90),
                selected: Color32::from_rgb(155, 76, 0),
                enclosure: Color32::from_rgb(29, 85, 158),
            }
        }
    }
}
const LIMIT: i32 = 99_990;

pub struct View {
    pub focus_id: Option<egui::Id>,
    pub request_focus: bool,
    pub cursor: Option<Point>,
    pointer: Option<Pos2>,
    pub drag: Option<Drag>,
    pub(crate) last_value_click: Option<(usize, f64)>,
    pan: Vec2,
    zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            focus_id: None,
            request_focus: true,
            cursor: None,
            pointer: None,
            drag: None,
            last_value_click: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl View {
    pub fn center_on(&mut self, point: Point) {
        self.pan = -vec2(point.0 as f32, point.1 as f32) * self.grid();
    }

    pub fn zoom_percent(&self) -> f32 {
        self.zoom * 100.0
    }

    fn grid(&self) -> f32 {
        self.zoom * 22.0
    }
}

pub enum Drag {
    Move {
        start: Point,
        delta: Point,
    },
    Box {
        start: Pos2,
        end: Pos2,
        additive: bool,
    },
}

#[derive(Clone, Copy)]
struct Transform {
    palette: Palette,
    origin: Pos2,
    grid: f32,
}

impl Transform {
    fn screen(self, p: Point) -> Pos2 {
        self.origin + vec2(p.0 as f32, p.1 as f32) * self.grid
    }
    fn snap(self, p: Pos2) -> Point {
        let v = (p - self.origin) / self.grid;
        Point(
            (v.x.round() as i32).clamp(-LIMIT, LIMIT),
            (v.y.round() as i32).clamp(-LIMIT, LIMIT),
        )
    }
}

fn component_bounds(t: Transform, c: &Component, delta: Point) -> Rect {
    let at = t.screen(c.at.offset(delta));
    let a = t.screen(c.terminal(0).offset(delta));
    let b = if c.kind == Kind::Ground {
        at
    } else {
        t.screen(c.terminal(1).offset(delta))
    };
    Rect::from_two_pos(a, b).expand(t.grid * 0.55)
}

fn wire_bounds(t: Transform, w: Wire) -> Rect {
    Rect::from_two_pos(t.screen(w.0), t.screen(w.1))
}

fn caption(c: &Component) -> String {
    let value = c
        .kind
        .unit()
        .map(|unit| format!("{} {unit}", display_metric(c.value)));
    match (c.label.is_empty(), value) {
        (true, Some(value)) => value,
        (false, Some(value)) => format!("{} · {value}", c.label),
        (false, None) => c.label.clone(),
        (true, None) => String::new(),
    }
}

fn caption_pos(t: Transform, c: &Component, delta: Point) -> Pos2 {
    t.screen(c.at.offset(delta)) + vec2(if c.rotation % 2 == 1 { 1.5 } else { 0.0 }, 0.78) * t.grid
}

fn caption_font(component: &Component) -> FontId {
    if component.kind.unit().is_some() {
        FontId::monospace(12.0)
    } else {
        FontId::proportional(12.0)
    }
}

fn caption_bounds(painter: &Painter, t: Transform, c: &Component) -> Rect {
    let galley = painter.layout_no_wrap(caption(c), caption_font(c), t.palette.ink);
    Align2::CENTER_TOP.anchor_size(caption_pos(t, c, Point::default()), galley.size())
}

fn displayed_component<'a>(
    editor: &'a Editor,
    index: usize,
    design: &'a Component,
) -> &'a Component {
    if crate::simulation::is_live_source(design.kind) {
        editor
            .latest_sample()
            .map_or(design, |sample| &sample.circuit.components[index])
    } else {
        design
    }
}

fn hit(editor: &Editor, painter: &Painter, t: Transform, pos: Pos2) -> Option<Item> {
    for (i, c) in editor.circuit.components.iter().enumerate().rev() {
        if component_bounds(t, c, Point::default()).contains(pos)
            || caption_bounds(painter, t, displayed_component(editor, i, c)).contains(pos)
        {
            return Some(Item::Component(i));
        }
    }
    editor
        .circuit
        .wires
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, &w)| (segment_distance(t, w, pos) <= 5.0).then_some(i))
        .and_then(|index| wiring::wire_path(&editor.circuit, index).into_iter().min())
        .map(Item::Wire)
}

fn segment_distance(t: Transform, wire: Wire, pos: Pos2) -> f32 {
    let start = t.screen(wire.0);
    let direction = t.screen(wire.1) - start;
    let along = ((pos - start).dot(direction) / direction.length_sq()).clamp(0.0, 1.0);
    pos.distance(start + direction * along)
}

fn target_at(
    circuit: &cirtam_core::circuit::Circuit,
    t: Transform,
    point: Point,
    pos: Pos2,
) -> Option<Target> {
    let links = circuit.connections.as_ref()?;
    let mut nodes = links
        .endpoints(circuit)
        .filter_map(|(id, p)| (p == point).then_some(id));
    if let Some(first) = nodes.next() {
        if nodes.all(|id| id == first)
            && !circuit
                .wires
                .iter()
                .any(|wire| wire.contains(point) && wire.0 != point && wire.1 != point)
        {
            return Some(Target::Node(first, point));
        }
    }
    let mut candidates: Vec<_> = circuit
        .wires
        .iter()
        .enumerate()
        .filter_map(|(i, &wire)| {
            if !wire.contains(point) {
                return None;
            }
            let [a, b] = links.wires[i];
            let target = if wire.0 == point {
                Target::Node(a, point)
            } else if wire.1 == point {
                Target::Node(b, point)
            } else {
                Target::Interior(i, point)
            };
            Some((segment_distance(t, wire, pos), target))
        })
        .collect();
    if let Some(first) = links
        .endpoints(circuit)
        .find_map(|(node, p)| (p == point).then_some(node))
    {
        if !candidates
            .iter()
            .any(|&(_, target)| target == Target::Node(first, point))
            && links
                .endpoints(circuit)
                .all(|(node, p)| p != point || node == first)
        {
            candidates.push((pos.distance(t.screen(point)), Target::Node(first, point)));
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let &(distance, target) = candidates.first()?;
    candidates
        .iter()
        .skip(1)
        .all(|&(other, candidate)| other - distance > 0.6 || candidate == target)
        .then_some(target)
}

// Distinct IDs at the same point are separate until the user reconnects them.
fn overlapping_nodes(circuit: &Circuit, point: Point) -> Vec<Target> {
    let Some(links) = circuit.connections.as_ref() else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for (node, position) in links.endpoints(circuit) {
        let target = Target::Node(node, point);
        if position == point && !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

fn apply_route_target(editor: &mut Editor, target: Target) {
    match editor.tool {
        Tool::Select | Tool::Wire(None) => {
            editor.tool = Tool::Wire(Some(target));
            editor.route_corners.clear();
            editor.notice.clear();
        }
        Tool::Wire(Some(start)) => {
            let previous = editor
                .route_corners
                .last()
                .copied()
                .unwrap_or(start.point());
            if target == start || (target.point() == previous && !editor.route_corners.is_empty()) {
                editor.notice = "Choose a different point.".into();
            } else {
                let corners = std::mem::take(&mut editor.route_corners);
                match editor.try_change(|c| wiring::connect(c, start, &corners, target)) {
                    Ok(()) => {
                        editor.tool = Tool::Select;
                        editor.selection.clear();
                        editor.notice.clear();
                    }
                    Err(error) => {
                        editor.route_corners = corners;
                        editor.notice = error;
                    }
                }
            }
        }
        _ => {}
    }
}

fn node_hit(editor: &Editor, t: Transform, pos: Pos2) -> Option<Item> {
    let point = t.snap(pos);
    if pos.distance(t.screen(point)) > 7.0 {
        return None;
    }
    match target_at(&editor.circuit, t, point, pos)? {
        Target::Node(id, _)
            if !editor.circuit.connections.as_ref()?.bends.contains(&id)
                && wiring::counts(&editor.circuit)[&id].1 > 1 =>
        {
            Some(Item::Node(id))
        }
        _ => None,
    }
}

fn segment_intersects(t: Transform, wire: Wire, rect: Rect) -> bool {
    let a = t.screen(wire.0);
    let b = t.screen(wire.1);
    let mut near = 0.0_f32;
    let mut far = 1.0_f32;
    for (start, delta, min, max) in [
        (a.x, b.x - a.x, rect.left(), rect.right()),
        (a.y, b.y - a.y, rect.top(), rect.bottom()),
    ] {
        if delta == 0.0 {
            if !(min..=max).contains(&start) {
                return false;
            }
        } else {
            let p = (min - start) / delta;
            let q = (max - start) / delta;
            near = near.max(p.min(q));
            far = far.min(p.max(q));
        }
    }
    near <= far
}

pub fn show(editor: &mut Editor, ui: &mut egui::Ui) {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    editor.view.focus_id = Some(response.id);
    if std::mem::take(&mut editor.view.request_focus)
        || response.clicked()
        || response.secondary_clicked()
    {
        response.request_focus();
    }
    let rect = response.rect;
    let center = rect.center();
    if response.dragged_by(egui::PointerButton::Middle) {
        editor.view.pan += ui.input(|i| i.pointer.delta());
    }
    if let Some(pos) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        let before = editor.view.zoom;
        editor.view.zoom = (before * (scroll * 0.002).exp()).clamp(0.45, 3.0);
        if editor.view.zoom != before {
            ui.ctx().request_repaint();
        }
        let anchor = pos - center;
        editor.view.pan = anchor + (editor.view.pan - anchor) * (editor.view.zoom / before);
    }
    let t = Transform {
        palette: Palette::for_theme(ui.visuals().dark_mode),
        origin: center + editor.view.pan,
        grid: editor.view.grid(),
    };
    let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    editor.view.pointer = response.hover_pos().filter(|_| !additive);
    editor.view.cursor = response.hover_pos().map(|p| t.snap(p));
    if response.drag_started()
        || response.secondary_clicked()
        || (ui.input(|input| input.pointer.any_pressed()) && !response.hovered())
    {
        editor.view.last_value_click = None;
    }
    let previous_value_click = if response.clicked() {
        editor.view.last_value_click.take()
    } else {
        None
    };
    let pointer = response.interact_pointer_pos();
    if let Some(pos) = pointer {
        let point = t.snap(pos);
        let item = hit(editor, &painter, t, pos);
        let target = target_at(&editor.circuit, t, point, pos);
        let overlapping = if response.clicked()
            && editor.design()
            && matches!(editor.tool, Tool::Select | Tool::Wire(None))
            && !additive
            && pos.distance(t.screen(point)) <= 7.0
        {
            overlapping_nodes(&editor.circuit, point)
        } else {
            Vec::new()
        };
        if overlapping.len() >= 2 {
            if response.double_clicked() && overlapping.len() == 2 {
                editor.notice = match editor
                    .try_change(|c| wiring::connect(c, overlapping[0], &[], overlapping[1]))
                {
                    Ok(()) => "Connections joined.".into(),
                    Err(error) => error,
                };
            } else if overlapping.len() == 2 {
                editor.notice = "Double-click to reconnect these nodes.".into();
            } else {
                editor.notice =
                    "More than two separate nodes overlap; no connection was made.".into();
            }
        } else if !editor.design() {
            if response.clicked() || response.secondary_clicked() {
                if let Some(Item::Component(i)) = item {
                    editor.open_details(i, false);
                }
            }
        } else if response.secondary_clicked() {
            if let Some(Item::Component(i)) = item {
                editor.open_details(i, false);
            }
        } else if response.clicked()
            && matches!(editor.tool, Tool::Select)
            && !additive
            && pos.distance(t.screen(point)) <= 7.0
            && target.is_some()
        {
            apply_route_target(editor, target.unwrap());
        } else if response.clicked() {
            match editor.tool {
                Tool::Place { kind, rotation } => {
                    let mut part = Component::new(kind, point);
                    part.rotation = rotation;
                    if let Err(error) = editor.try_change(|c| wiring::add_component(c, part)) {
                        editor.notice = error;
                    }
                }
                Tool::Wire(Some(start)) => {
                    let previous = editor
                        .route_corners
                        .last()
                        .copied()
                        .unwrap_or(start.point());
                    if let Some(end) = target {
                        apply_route_target(editor, end);
                    } else if point == previous {
                        editor.notice = "Choose a different point.".into();
                    } else if editor.circuit.wires.iter().any(|w| w.contains(point)) {
                        editor.notice = "Several wires meet visually here. Click toward the wire you want to connect.".into();
                    } else {
                        editor.route_corners.push(point);
                        editor.notice.clear();
                    }
                }
                Tool::Wire(None) => {
                    if let Some(start) = target {
                        apply_route_target(editor, start);
                    } else {
                        editor.notice = "Choose a terminal or point on a wire.".into();
                    }
                }
                Tool::Select => {
                    if !additive {
                        editor.selection.clear();
                    }
                    if let Some(item) = if additive {
                        node_hit(editor, t, pos).or(item)
                    } else {
                        item
                    } {
                        if !editor.selection.insert(item) {
                            editor.selection.remove(&item);
                        }
                    }
                    if let Some(Item::Component(index)) = item {
                        let component = &editor.circuit.components[index];
                        if component.kind.unit().is_some()
                            && caption_bounds(&painter, t, component).contains(pos)
                        {
                            let now = ui.input(|input| input.time);
                            let delay = ui
                                .ctx()
                                .options(|options| options.input_options.max_double_click_delay);
                            if previous_value_click.is_some_and(|(previous, time)| {
                                previous == index && now - time < delay
                            }) {
                                editor.open_details(index, true);
                            } else {
                                editor.view.last_value_click = Some((index, now));
                            }
                        }
                    }
                }
            }
        }
        if editor.design() && matches!(editor.tool, Tool::Select) {
            if response.drag_started_by(egui::PointerButton::Primary) {
                let start = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                editor.view.drag = Some(
                    if let Some(item) =
                        node_hit(editor, t, start).or_else(|| hit(editor, &painter, t, start))
                    {
                        if !editor.selection.contains(&item) {
                            if !additive {
                                editor.selection.clear();
                            }
                            editor.selection.insert(item);
                        }
                        Drag::Move {
                            start: t.snap(start),
                            delta: Point::default(),
                        }
                    } else {
                        Drag::Box {
                            start,
                            end: pos,
                            additive,
                        }
                    },
                );
            }
            match &mut editor.view.drag {
                Some(Drag::Move { start, delta }) => {
                    *delta = Point(point.0 - start.0, point.1 - start.1)
                }
                Some(Drag::Box { end, .. }) => *end = pos,
                None => {}
            }
        }
    }
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        if let Some(drag) = editor.view.drag.take() {
            finish_drag(editor, &painter, t, drag);
        }
    }
    draw(editor, &painter, rect, t);
}

fn finish_drag(editor: &mut Editor, painter: &Painter, t: Transform, drag: Drag) {
    match drag {
        Drag::Move { delta, .. } => {
            let moved =
                wiring::translated(&editor.circuit, &editor.selection, delta).and_then(|circuit| {
                    circuit.validate_geometry()?;
                    Ok(circuit)
                });
            match moved {
                Ok(circuit) => editor.change(|c| *c = circuit),
                Err(e) => editor.notice = e,
            }
        }
        Drag::Box {
            start,
            end,
            additive,
        } => {
            if !additive {
                editor.selection.clear();
            }
            let selection = Rect::from_two_pos(start, end);
            let matches = |bounds| {
                if start.x <= end.x {
                    selection.contains_rect(bounds)
                } else {
                    selection.intersects(bounds)
                }
            };
            for (i, component) in editor.circuit.components.iter().enumerate() {
                let mut bounds = component_bounds(t, component, Point::default());
                if !caption(component).is_empty() {
                    bounds = bounds.union(caption_bounds(painter, t, component));
                }
                if matches(bounds) {
                    editor.selection.insert(Item::Component(i));
                }
            }
            let mut visited = std::collections::HashSet::new();
            for index in 0..editor.circuit.wires.len() {
                if visited.contains(&index) {
                    continue;
                }
                let path = wiring::wire_path(&editor.circuit, index);
                let mut segments = path.iter().map(|&index| editor.circuit.wires[index]);
                let selected = if start.x <= end.x {
                    segments.all(|wire| selection.contains_rect(wire_bounds(t, wire)))
                } else {
                    segments.any(|wire| segment_intersects(t, wire, selection))
                };
                visited.extend(path);
                if selected {
                    editor.selection.insert(Item::Wire(index));
                }
            }
        }
    }
}

fn pointer_is_near_node(editor: &Editor, t: Transform, point: Point) -> bool {
    editor.view.pointer.is_some_and(|pos| {
        pos.distance(t.screen(point)) <= 7.0 && target_at(&editor.circuit, t, point, pos).is_some()
    })
}

fn draw(editor: &Editor, painter: &Painter, rect: Rect, t: Transform) {
    painter.rect_filled(rect, 0.0, t.palette.background);
    let first_x = rect.left() + (t.origin.x - rect.left()).rem_euclid(t.grid);
    let first_y = rect.top() + (t.origin.y - rect.top()).rem_euclid(t.grid);
    let mut x = first_x;
    while x <= rect.right() {
        let mut y = first_y;
        while y <= rect.bottom() {
            painter.circle_filled(pos2(x, y), 1.0, t.palette.grid);
            y += t.grid;
        }
        x += t.grid;
    }
    let preview = match editor.view.drag {
        Some(Drag::Move { delta, .. }) => {
            wiring::translated(&editor.circuit, &editor.selection, delta).ok()
        }
        _ => None,
    };
    let circuit = preview.as_ref().unwrap_or(&editor.circuit);
    let highlights = diagnostic_highlights(editor, painter);
    let selected_segments = wiring::selected_segments(&editor.circuit, &editor.selection);
    for (i, &Wire(a, b)) in circuit.wires.iter().enumerate() {
        let item = Item::Wire(i);
        let color = if selected_segments.contains(&i) {
            t.palette.selected
        } else {
            t.palette.wire
        };
        let endpoints = [t.screen(a), t.screen(b)];
        let stroke_color = highlights
            .get(&item)
            .map_or(color, |&pulse| color.blend(pulse));
        painter.line_segment(endpoints, Stroke::new(2.0_f32, stroke_color));
    }
    for (i, c) in circuit.components.iter().enumerate() {
        let item = Item::Component(i);
        let color = if editor.selection.contains(&item) {
            t.palette.selected
        } else {
            t.palette.ink
        };
        draw_component(
            painter,
            t,
            displayed_component(editor, i, c),
            Point::default(),
            color,
            highlights.get(&item).copied(),
        );
        if let Some(solution) = editor.solution() {
            let reading = solution.readings[i];
            let result = match c.kind {
                Kind::Voltmeter => Some(format!("{} V", display_metric(reading.voltage))),
                Kind::Ammeter => reading
                    .current
                    .map(|current| format!("{} A", display_metric(current))),
                _ => None,
            };
            if let Some(result) = result {
                painter.text(
                    caption_pos(t, c, Point::default()) + vec2(0.0, 15.0),
                    Align2::CENTER_TOP,
                    result,
                    FontId::monospace(13.0),
                    t.palette.wire,
                );
            }
        }
    }
    let mut markers = HashMap::<Point, (bool, bool, usize)>::new();
    for (node, (point, count)) in wiring::counts(circuit) {
        let marker = markers.entry(point).or_default();
        marker.0 |= count > 1 && !circuit.connections.as_ref().unwrap().bends.contains(&node);
        marker.1 |= editor.selection.contains(&Item::Node(node));
        marker.2 += 1;
    }
    for (point, (connected, selected, coincident)) in markers {
        let pos = t.screen(point);
        let color = if selected {
            t.palette.selected
        } else {
            t.palette.wire
        };
        if connected {
            painter.circle_filled(pos, 3.6, color);
        }
        if coincident > 1 {
            painter.circle_stroke(pos, 6.0, Stroke::new(1.5_f32, t.palette.ink));
        }
    }
    if let Some(point) = editor.view.cursor {
        match editor.tool {
            Tool::Place { kind, rotation } => {
                let mut part = Component::new(kind, point);
                part.rotation = rotation;
                draw_component(
                    painter,
                    t,
                    &part,
                    Point::default(),
                    t.palette.wire.gamma_multiply(0.65),
                    None,
                );
            }
            Tool::Wire(start) => {
                if let Some(start) = start {
                    let mut previous = start.point();
                    painter.circle_filled(t.screen(previous), 4.0, t.palette.selected);
                    for next in editor.route_corners.iter().copied().chain([point]) {
                        painter.line_segment(
                            [t.screen(previous), t.screen(next)],
                            Stroke::new(1.5_f32, t.palette.selected),
                        );
                        previous = next;
                    }
                    for &corner in &editor.route_corners {
                        painter.circle_filled(t.screen(corner), 2.5, t.palette.selected);
                    }
                }
                if editor
                    .view
                    .pointer
                    .is_some_and(|pos| target_at(&editor.circuit, t, point, pos).is_some())
                {
                    painter.circle_stroke(
                        t.screen(point),
                        6.0,
                        Stroke::new(2.0_f32, t.palette.selected),
                    );
                }
            }
            Tool::Select
                if editor.design()
                    && editor.view.drag.is_none()
                    && pointer_is_near_node(editor, t, point) =>
            {
                painter.circle_stroke(t.screen(point), 6.0, Stroke::new(1.5_f32, t.palette.wire));
            }
            Tool::Select => {}
        }
    }
    if let Some(Drag::Box { start, end, .. }) = editor.view.drag {
        let color = if start.x <= end.x {
            t.palette.enclosure
        } else {
            t.palette.wire
        };
        let area = Rect::from_two_pos(start, end);
        painter.rect_filled(area, 0.0, color.gamma_multiply(0.12));
        painter.rect_stroke(area, 0.0, Stroke::new(1.0_f32, color), StrokeKind::Inside);
    }
}

fn diagnostic_highlights(editor: &Editor, painter: &Painter) -> HashMap<Item, Color32> {
    let mut severities = HashMap::<Item, Severity>::new();
    for diagnostic in editor.current_diagnostics() {
        let item = match diagnostic.location {
            Some(Location::Component(index)) => Item::Component(index),
            Some(Location::Wire(index)) => Item::Wire(index),
            None => continue,
        };
        let severity = severities.entry(item).or_insert(diagnostic.severity);
        *severity = (*severity).max(diagnostic.severity);
    }
    if severities.is_empty() {
        return HashMap::new();
    }
    painter
        .ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let time = painter.ctx().input(|input| input.time);
    let alpha = (130.0 + 70.0 * (time * std::f64::consts::TAU / 2.2).sin()) as u8;
    severities
        .into_iter()
        .map(|(item, severity)| {
            let color = match severity {
                Severity::Serious => Color32::from_rgba_unmultiplied(255, 50, 70, alpha),
                Severity::Warning => Color32::from_rgba_unmultiplied(238, 184, 0, alpha),
            };
            (item, color)
        })
        .collect()
}

fn draw_component(
    painter: &Painter,
    t: Transform,
    c: &Component,
    delta: Point,
    color: Color32,
    pulse: Option<Color32>,
) {
    let center = t.screen(c.at.offset(delta));
    let axis = (center - t.screen(c.terminal(0).offset(delta))).normalized();
    let perpendicular = vec2(-axis.y, axis.x);
    let local = |x, y| center + (axis * x + perpendicular * y) * t.grid;
    let stroke = Stroke::new(1.8_f32, pulse.map_or(color, |pulse| color.blend(pulse)));
    let line = |a, b| {
        painter.line_segment([a, b], stroke);
    };
    if c.kind == Kind::Ground {
        line(local(-2.0, 0.0), local(-0.25, 0.0));
        for (x, width) in [(-0.25, 0.5), (0.02, 0.34), (0.27, 0.16)] {
            line(local(x, -width), local(x, width));
        }
    } else {
        line(local(-2.0, 0.0), local(-0.65, 0.0));
        line(local(0.65, 0.0), local(2.0, 0.0));
        if c.kind == Kind::Resistor {
            let mut points = vec![local(-0.65, 0.0)];
            for i in 0..6 {
                points.push(local(
                    -0.55 + i as f32 * 0.22,
                    if i % 2 == 0 { -0.29 } else { 0.29 },
                ));
            }
            points.push(local(0.65, 0.0));
            painter.add(egui::Shape::line(points, stroke));
        } else {
            painter.circle(center, 0.65 * t.grid, t.palette.background, stroke);
            match c.kind {
                Kind::Current => {
                    line(local(-0.38, 0.0), local(0.38, 0.0));
                    line(local(0.38, 0.0), local(0.14, -0.19));
                    line(local(0.38, 0.0), local(0.14, 0.19));
                }
                Kind::Voltage => {
                    line(local(-0.42, 0.0), local(-0.13, 0.0));
                    line(local(-0.275, -0.15), local(-0.275, 0.15));
                    line(local(0.13, 0.0), local(0.42, 0.0));
                }
                Kind::Voltmeter | Kind::Ammeter => {
                    let symbol = if c.kind == Kind::Voltmeter { "V" } else { "A" };
                    painter.text(
                        center,
                        Align2::CENTER_CENTER,
                        symbol,
                        FontId::proportional(t.grid * 0.72),
                        color,
                    );
                }
                _ => {}
            }
        }
    }
    for (i, point) in c.terminals().enumerate() {
        let pos = t.screen(point.offset(delta));
        painter.circle(pos, 3.2, t.palette.background, Stroke::new(1.8_f32, color));
        if c.kind != Kind::Ground {
            painter.text(
                pos + perpendicular * 6.0,
                Align2::CENTER_TOP,
                (i + 1).to_string(),
                FontId::proportional(9.0),
                color,
            );
        }
    }
    painter.text(
        caption_pos(t, c, delta),
        Align2::CENTER_TOP,
        caption(c),
        caption_font(c),
        color,
    );
}
