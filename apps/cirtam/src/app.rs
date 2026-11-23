use crate::wiring::{self, Target};
use cirtam_core::topology::Node;
use cirtam_core::{
    circuit::{Circuit, Component, Kind, Point, Ratings},
    diagnostics::{Diagnostic, Location, Severity},
    persistence::{self, CIRTAM_EXTENSION},
    solver::{self, Solution},
};
use eframe::egui::{self, Key};
use std::{
    collections::HashSet,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

const CIRCUIT_CLIPBOARD_MARKER: &str = "Cirtam circuit selection";

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    Component(usize),
    Wire(usize),
    Node(Node),
}

#[derive(Clone, Copy)]
pub enum Tool {
    Select,
    Place { kind: Kind, rotation: u8 },
    Wire(Option<Target>),
}

pub enum Simulation {
    Design,
    Results { solution: Solution },
}

pub struct Details {
    index: usize,
    label: String,
    value: String,
    power_rating: String,
    tolerance: String,
    focus_value: bool,
    error: Option<String>,
}

pub struct Editor {
    pub circuit: Circuit,
    pub selection: HashSet<Item>,
    pub tool: Tool,
    pub route_corners: Vec<Point>,
    clipboard: Option<Circuit>,
    pub simulation: Simulation,
    cpu: crate::telemetry::CpuUsage,
    last_solve: Option<Duration>,
    pub view: crate::canvas::View,
    pub notice: String,
    pub diagnostics: Vec<Diagnostic>,
    undo: Vec<Circuit>,
    redo: Vec<Circuit>,
    saved: Circuit,
    path: Option<PathBuf>,
    palette: Option<crate::palette::Search>,
    search: String,
    details: Option<Details>,
    pending: Option<FileAction>,
    allow_close: bool,
}

#[derive(Clone, Copy)]
enum FileAction {
    New,
    Open,
    Close,
}

impl Default for Editor {
    fn default() -> Self {
        let circuit = Circuit {
            connections: Some(Default::default()),
            ..Default::default()
        };
        Self {
            circuit: circuit.clone(),
            selection: HashSet::new(),
            tool: Tool::Select,
            simulation: Simulation::Design,
            cpu: Default::default(),
            last_solve: None,
            view: Default::default(),
            notice: String::new(),
            diagnostics: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            saved: circuit,
            route_corners: Vec::new(),
            clipboard: None,
            path: None,
            palette: None,
            search: String::new(),
            details: None,
            pending: None,
            allow_close: false,
        }
    }
}

impl Editor {
    pub fn design(&self) -> bool {
        matches!(self.simulation, Simulation::Design)
    }

    fn routing(&self) -> bool {
        matches!(self.tool, Tool::Wire(Some(_)))
    }

    pub fn change(&mut self, edit: impl FnOnce(&mut Circuit)) {
        let old = self.circuit.clone();
        edit(&mut self.circuit);
        self.record_change(old);
    }

    pub fn try_change(
        &mut self,
        edit: impl FnOnce(&mut Circuit) -> Result<(), String>,
    ) -> Result<(), String> {
        let old = self.circuit.clone();
        if let Err(error) = edit(&mut self.circuit) {
            self.circuit = old;
            return Err(error);
        }
        self.record_change(old);
        Ok(())
    }

    fn record_change(&mut self, old: Circuit) {
        if self.circuit != old {
            self.view.last_value_click = None;
            self.diagnostics.clear();
            self.undo.push(old);
            self.redo.clear();
        }
    }

    fn history(&mut self, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        if let Some(circuit) = from.pop() {
            to.push(std::mem::replace(&mut self.circuit, circuit));
            self.clear_interaction();
            self.diagnostics.clear();
        }
    }

    fn clear_interaction(&mut self) {
        self.selection.clear();
        self.tool = Tool::Select;
        self.route_corners.clear();
        self.details = None;
        self.view.drag = None;
        self.view.last_value_click = None;
        self.palette = None;
    }

    fn delete(&mut self) {
        let selection = std::mem::take(&mut self.selection);
        match self.try_change(|c| wiring::delete(c, &selection)) {
            Ok(()) => self.details = None,
            Err(error) => {
                self.selection = selection;
                self.notice = error;
            }
        }
    }

    fn orient(&mut self, orientation: impl Fn(u8) -> u8) {
        if let Tool::Place { rotation, .. } = &mut self.tool {
            *rotation = orientation(*rotation);
            self.view.request_focus = true;
            return;
        }
        let mut rotated = self.circuit.clone();
        for (i, part) in rotated.components.iter_mut().enumerate() {
            if self.selection.contains(&Item::Component(i)) {
                part.rotation = orientation(part.rotation);
            }
        }
        let result = wiring::follow_nodes(&self.circuit, &mut rotated, &Default::default())
            .and_then(|()| rotated.validate_geometry());
        match result {
            Ok(()) => self.change(|c| *c = rotated),
            Err(error) => self.notice = error,
        }
    }

    fn run(&mut self) {
        if !self.design() {
            return;
        }
        if self.routing() {
            self.notice = "Finish the wire or press Esc to cancel before running.".into();
            return;
        }
        let start = Instant::now();
        let result = solver::solve(&self.circuit);
        self.last_solve = Some(start.elapsed());
        match result {
            Ok(mut solution) => {
                self.diagnostics = std::mem::take(&mut solution.diagnostics);
                self.clear_interaction();
                self.notice.clear();
                self.simulation = Simulation::Results { solution };
            }
            Err(diagnostics) => {
                self.notice.clear();
                self.diagnostics = diagnostics;
            }
        }
    }

    fn stop(&mut self) {
        if !self.design() {
            self.simulation = Simulation::Design;
            self.clear_interaction();
        }
        self.notice.clear();
    }

    pub fn open_details(&mut self, index: usize, focus_value: bool) {
        let c = &self.circuit.components[index];
        self.details = Some(Details {
            index,
            label: c.label.clone(),
            value: c.value.to_string(),
            power_rating: c
                .ratings
                .power_watts
                .map(|value| value.to_string())
                .unwrap_or_default(),
            tolerance: c
                .ratings
                .tolerance_percent
                .map(|value| value.to_string())
                .unwrap_or_default(),
            focus_value,
            error: None,
        });
    }

    fn details_window(&mut self, ctx: &egui::Context) {
        let Some(mut details) = self.details.take() else {
            return;
        };
        let c = &self.circuit.components[details.index];
        let kind = c.kind;
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(kind.name())
            .id(egui::Id::new("component-details"))
            .default_pos(ctx.available_rect().right_top() + egui::vec2(-295.0, 24.0))
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                if self.design() {
                    ui.horizontal(|ui| {
                        ui.label("Label");
                        ui.text_edit_singleline(&mut details.label);
                    });
                    if let Some(unit) = kind.unit() {
                        ui.horizontal(|ui| {
                            ui.label("Value");
                            let input = ui.add(
                                egui::TextEdit::singleline(&mut details.value)
                                    .font(egui::TextStyle::Monospace),
                            );
                            if details.focus_value {
                                input.request_focus();
                                details.focus_value = false;
                            }
                            apply |= input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            ui.monospace(unit);
                        });
                    }
                    if kind == Kind::Resistor {
                        for (label, field, unit) in [
                            ("Power rating", &mut details.power_rating, "W"),
                            ("Tolerance", &mut details.tolerance, "±%"),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(label);
                                let input = ui.add(
                                    egui::TextEdit::singleline(field)
                                        .hint_text("Unspecified")
                                        .font(egui::TextStyle::Monospace),
                                );
                                apply |=
                                    input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                                ui.monospace(unit);
                            });
                        }
                        ui.label("Changing nominal resistance or tolerance samples a new actual resistance.");
                    } else {
                        if kind.unit().is_none() && c.value != 0.0 {
                            ui.label(format!("Apply will reset the stored value from {} to 0.", c.value));
                        }
                        for (field, value) in [
                            ("power rating (W)", c.ratings.power_watts),
                            ("tolerance (%)", c.ratings.tolerance_percent),
                            ("actual resistance (Ω)", c.actual_ohms),
                        ] {
                            if let Some(value) = value {
                                ui.label(format!("Apply will remove the resistor-only {field} of {value}."));
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        apply |= ui.button("Apply").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                    if let Some(error) = &details.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                } else if let Simulation::Results { solution, .. } = &self.simulation {
                    if !c.label.is_empty() {
                        ui.label(&c.label);
                    }
                    let reading = solution.readings[details.index];
                    ui.horizontal(|ui| {
                        ui.label("Voltage");
                        ui.monospace(format!("{} V", display_number(reading.voltage)));
                    });
                    if let Some(current) = reading.current {
                        ui.horizontal(|ui| {
                            ui.label("Current");
                            ui.monospace(format!("{} A", display_number(current)));
                        });
                        let power = reading.resistor_power.unwrap_or(reading.voltage * current);
                        ui.horizontal(|ui| {
                            ui.label(if kind == Kind::Resistor {
                                "DC dissipation"
                            } else {
                                "DC power"
                            });
                            if power.is_finite() {
                                ui.monospace(format!("{} W", display_number(power)));
                            } else {
                                ui.label("Out of range");
                            }
                        })
                        .response
                        .on_hover_text("Positive power is absorbed. Negative power is delivered.");
                        if let Some(power) = reading.resistor_power {
                            show_resistor_results(ui, c, power);
                        }
                        ui.label("Positive current flows from terminal 1 to terminal 2.");
                    } else {
                        ui.label("Current and power are undefined at the ground reference.");
                    }
                }
                match kind {
                    Kind::Voltmeter => {
                        ui.label("Ideal open circuit. Place across two nodes.");
                    }
                    Kind::Ammeter => {
                        ui.label("Ideal 0 V drop. Insert in series.");
                    }
                    _ => {}
                }
            });
        if open && !cancel {
            if apply {
                self.apply_details(&mut details);
            }
            self.details = Some(details);
        }
    }

    fn apply_details(&mut self, details: &mut Details) {
        let component = &self.circuit.components[details.index];
        let kind = component.kind;
        let candidate = kind.unit().map_or(Ok(0.0), |_| kind.parse_value(&details.value))
            .and_then(|value| {
                let ratings = if kind == Kind::Resistor {
                    Ratings {
                        power_watts: optional_rating(&details.power_rating, "Power rating")?,
                        tolerance_percent: optional_rating(&details.tolerance, "Tolerance")?,
                    }
                } else {
                    Ratings::default()
                };
                let mut updated = component.clone();
                crate::resistance::edit_values(&mut updated, value, ratings)?;
                updated.validate_values().map_err(|error| {
                    if kind == Kind::Resistor && updated.actual_ohms.is_some() {
                        format!("{error} Change the nominal resistance or tolerance to replace or clear the stored sample.")
                    } else {
                        error
                    }
                })?;
                updated.label.clone_from(&details.label);
                Ok(updated)
            });
        match candidate {
            Ok(updated) => {
                self.change(|circuit| circuit.components[details.index] = updated);
                details.error = None;
            }
            Err(error) => details.error = Some(error),
        }
    }

    fn save(&mut self, save_as: bool) -> bool {
        let path = if save_as || self.path.is_none() {
            let Some(mut path) = rfd::FileDialog::new()
                .add_filter("Cirtam circuit", &[CIRTAM_EXTENSION])
                .set_file_name("untitled.crtm")
                .save_file()
            else {
                return false;
            };
            if path.extension().is_none() {
                path.set_extension(CIRTAM_EXTENSION);
            }
            path
        } else {
            self.path.clone().unwrap()
        };
        let result = persistence::encode(&self.circuit)
            .and_then(|text| write_save(&path, &text).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.notice = format!("Saved {}", path.display());
                self.path = Some(path);
                self.saved = self.circuit.clone();
                true
            }
            Err(error) => {
                self.notice = format!("Save failed. {error}");
                false
            }
        }
    }

    fn file_action(&mut self, action: FileAction) {
        if self.saved != self.circuit {
            self.pending = Some(action);
        } else {
            self.execute_file_action(action);
        }
    }

    fn execute_file_action(&mut self, action: FileAction) {
        let (mut circuit, path) = match action {
            FileAction::Close => return,
            FileAction::New => (Circuit::default(), None),
            FileAction::Open => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Cirtam circuit", &[CIRTAM_EXTENSION])
                    .pick_file()
                else {
                    return;
                };
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|text| persistence::decode(&text))
                {
                    Ok(circuit) => (circuit, Some(path)),
                    Err(error) => {
                        self.notice = format!("Open failed. {error}");
                        return;
                    }
                }
            }
        };
        circuit.make_connections_explicit();
        self.saved = circuit.clone();
        self.circuit = circuit;
        self.diagnostics.clear();
        self.path = path;
        self.undo.clear();
        self.redo.clear();
        self.clear_interaction();
        self.view = Default::default();
        self.notice.clear();
    }

    fn place(&mut self, kind: Kind) {
        self.tool = Tool::Place { kind, rotation: 0 };
        self.view.request_focus = true;
    }

    fn activate(&mut self, action: crate::palette::Action) {
        use crate::palette::Action;
        match action {
            Action::Select => self.tool = Tool::Select,
            Action::Wire => self.tool = Tool::Wire(None),
            Action::Run => self.run(),
            Action::Stop => self.stop(),
            Action::Place(kind) => self.place(kind),
        }
    }

    fn can_orient(&self) -> bool {
        matches!(self.tool, Tool::Place { .. })
            || self
                .selection
                .iter()
                .any(|item| matches!(item, Item::Component(_)))
    }

    fn other_keyboard_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|memory| {
            memory
                .focused()
                .is_some_and(|id| Some(id) != self.view.focus_id)
        })
    }

    fn canvas_keys(&mut self, ctx: &egui::Context) {
        let canvas_focused = ctx.memory(|memory| {
            memory
                .focused()
                .is_some_and(|id| Some(id) == self.view.focus_id)
        });
        if self.palette.is_none() && self.pending.is_none() && canvas_focused && !self.routing() {
            let text = ctx.input(|input| {
                if input.modifiers.command
                    || input.key_pressed(Key::Escape)
                    || (self.design() && self.can_orient() && input.key_pressed(Key::R))
                {
                    return String::new();
                }
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>()
            });
            if !text.trim().is_empty() {
                self.palette = Some(crate::palette::Search::new(text));
            }
        }
        if self.palette.is_none() {
            self.shortcuts(ctx);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.other_keyboard_focus(ctx) || self.pending.is_some() {
            return;
        }
        let pressed = |key| {
            ctx.input(|i| i.events.iter().any(|event|
            matches!(event, egui::Event::Key { key: event_key, pressed: true, repeat: false, .. } if *event_key == key)))
        };
        if pressed(Key::Space) {
            self.run();
        }
        if pressed(Key::Escape) {
            if matches!(self.tool, Tool::Wire(_)) && self.design() {
                self.tool = Tool::Select;
                self.route_corners.clear();
                self.notice.clear();
            } else if self.design() {
                self.clear_interaction();
            } else {
                self.stop();
            }
        }
        if !self.design() || self.routing() {
            return;
        }
        if pressed(Key::R) && self.can_orient() {
            self.orient(|rotation| (rotation + 1) % 4);
        }
        if pressed(Key::Delete) {
            self.delete();
        }
        let modifiers = ctx.input(|i| i.modifiers);
        let (copy_event, paste_event) = ctx.input(|i| (
            i.events.iter().any(|event| matches!(event, egui::Event::Copy)),
            i.events.iter().any(|event| matches!(event, egui::Event::Paste(text) if text == CIRCUIT_CLIPBOARD_MARKER)),
        ));
        if copy_event || (modifiers.command && pressed(Key::C)) {
            self.copy(ctx);
        }
        if paste_event || (modifiers.command && pressed(Key::V)) {
            self.paste();
        }
        if let Some(save_as) = ctx.input(|i| save_shortcut(&i.events)) {
            self.save(save_as);
        }
        let (wire, history_z, history_y) = ctx.input(|input| {
            let mut wire = false;
            let mut history_z = None;
            let mut history_y = false;
            for event in &input.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } = event
                {
                    if modifiers.command {
                        match key {
                            Key::W if modifiers.shift && !modifiers.alt => wire = true,
                            Key::Z => {
                                history_z.get_or_insert(modifiers.shift);
                            }
                            Key::Y => history_y = true,
                            _ => {}
                        }
                    }
                }
            }
            (wire, history_z, history_y)
        });
        if wire && !matches!(self.tool, Tool::Wire(_)) {
            self.tool = Tool::Wire(None);
        }
        if let Some(redo) = history_z {
            self.history(redo);
        }
        if history_y {
            self.history(true);
        }
        if modifiers.command {
            if pressed(Key::O) {
                self.file_action(FileAction::Open);
            }
        }
    }

    fn top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Cirtam");
                ui.separator();
                ui.add_enabled_ui(self.design() && !self.routing(), |ui| {
                    if ui.button("New").clicked() {
                        self.file_action(FileAction::New);
                    }
                    if ui.button("Open").clicked() {
                        self.file_action(FileAction::Open);
                    }
                    if ui.button("Save").clicked() {
                        self.save(false);
                    }
                    if ui.button("Save as").clicked() {
                        self.save(true);
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        self.history(false);
                    }
                    if ui
                        .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        self.history(true);
                    }
                });
                if ui
                    .add_enabled(self.design(), egui::Button::new("Run"))
                    .clicked()
                {
                    self.run();
                }
                if ui
                    .add_enabled(!self.design(), egui::Button::new("Stop"))
                    .clicked()
                {
                    self.stop();
                }
                let mut dark = ctx.theme() == egui::Theme::Dark;
                if ui.toggle_value(&mut dark, "Dark mode").changed() {
                    ctx.set_theme(if dark {
                        egui::Theme::Dark
                    } else {
                        egui::Theme::Light
                    });
                    if let Err(error) = crate::save_theme(dark) {
                        self.notice = format!("Theme preference could not be saved. {error}");
                    }
                }
            });
            if self.design() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!self.routing(), |ui| self.design_tools(ui));
                });
            }
        });
    }

    fn copy(&mut self, ctx: &egui::Context) {
        let copy = wiring::copy(&self.circuit, &self.selection);
        if copy.components.is_empty() && copy.wires.is_empty() {
            self.notice = "Select components or wires to copy.".into();
        } else {
            self.clipboard = Some(copy);
            ctx.copy_text(CIRCUIT_CLIPBOARD_MARKER.into());
            self.notice = "Selection copied.".into();
        }
    }

    fn paste(&mut self) {
        let Some(copy) = &self.clipboard else {
            self.notice = "Copy components or wires first.".into();
            return;
        };
        let mut pasted = self.circuit.clone();
        let first_component = pasted.components.len();
        let result = wiring::paste(&mut pasted, copy).and_then(|items| {
            for component in &mut pasted.components[first_component..] {
                crate::resistance::resample(component)?;
            }
            Ok(items)
        });
        match result {
            Ok(items) => {
                let anchor = pasted
                    .components
                    .get(self.circuit.components.len())
                    .map(|c| c.at)
                    .or_else(|| pasted.wires.get(self.circuit.wires.len()).map(|w| w.0));
                self.change(|c| *c = pasted);
                self.clear_interaction();
                self.selection = items;
                if let Some(point) = anchor {
                    self.view.center_on(point);
                }
                self.notice.clear();
            }
            Err(error) => self.notice = error,
        }
    }

    fn design_tools(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(matches!(self.tool, Tool::Select), "Select")
            .clicked()
        {
            self.tool = Tool::Select;
        }
        if ui
            .selectable_label(matches!(self.tool, Tool::Wire(_)), "Wire")
            .on_hover_text(if cfg!(target_os = "macos") {
                "⌘⇧W"
            } else {
                "Ctrl+Shift+W"
            })
            .clicked()
            && !matches!(self.tool, Tool::Wire(_))
        {
            self.tool = Tool::Wire(None);
        }
        ui.add_enabled_ui(self.can_orient(), |ui| {
            if ui.button("Rotate (R)").clicked() {
                self.orient(|rotation| (rotation + 1) % 4);
            }
            ui.menu_button("Mirror", |ui| {
                // All current symbols are symmetric across their terminal axis.
                // Reflections therefore fit in the existing four orientations.
                if ui.button("Horizontal (left ↔ right)").clicked() {
                    self.orient(|rotation| (6 - rotation) % 4);
                    ui.close_menu();
                }
                if ui.button("Vertical (up ↔ down)").clicked() {
                    self.orient(|rotation| (4 - rotation) % 4);
                    ui.close_menu();
                }
            });
        });
        let copyable = self
            .selection
            .iter()
            .any(|item| !matches!(item, Item::Node(_)));
        if ui
            .add_enabled(copyable, egui::Button::new("Copy"))
            .clicked()
        {
            self.copy(ui.ctx());
        }
        if ui
            .add_enabled(self.clipboard.is_some(), egui::Button::new("Paste"))
            .clicked()
        {
            self.paste();
        }
        if let Some(&Item::Node(node)) = self
            .selection
            .iter()
            .next()
            .filter(|_| self.selection.len() == 1)
        {
            if ui
                .button("Simplify bend")
                .on_hover_text("Treat two segments as one wire and keep the bend in place")
                .clicked()
            {
                let mut simplified = self.circuit.clone();
                if wiring::simplify(&mut simplified, node) {
                    self.change(|c| *c = simplified);
                    self.selection.clear();
                    self.notice.clear();
                } else {
                    self.notice = "Simplify requires exactly two wire segments and no component terminal at the node.".into();
                }
            }
        }
        if ui
            .add_enabled(!self.selection.is_empty(), egui::Button::new("Delete"))
            .on_hover_text("Delete selected objects. A selected connection node is disconnected.")
            .clicked()
        {
            self.delete();
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        if !self.design() {
            return;
        }
        egui::SidePanel::left("components")
            .exact_width(200.0)
            .show(ctx, |ui| {
                ui.heading("Components");
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search components"));
                let query = self.search.trim().to_lowercase();
                for (category, kinds) in [
                    ("Passives", &[Kind::Resistor][..]),
                    ("Sources", &[Kind::Voltage, Kind::Current][..]),
                    ("Reference", &[Kind::Ground][..]),
                    ("Meters", &[Kind::Voltmeter, Kind::Ammeter][..]),
                ] {
                    let matching: Vec<_> = kinds
                        .iter()
                        .filter(|k| k.name().to_lowercase().contains(&query))
                        .collect();
                    if matching.is_empty() {
                        continue;
                    }
                    ui.add_space(8.0);
                    ui.strong(category);
                    for &kind in matching {
                        if ui
                            .add_enabled_ui(!self.routing(), |ui| {
                                ui.selectable_label(
                                    matches!(self.tool, Tool::Place { kind: k, .. } if k == kind),
                                    kind.name(),
                                )
                            })
                            .inner
                            .clicked()
                        {
                            self.place(kind);
                        }
                    }
                }
            });
    }

    fn diagnostics_window(&mut self, ctx: &egui::Context, workspace: egui::Rect) {
        if self.diagnostics.is_empty() {
            return;
        }
        let mut focus = None;
        let height = (workspace.height() * 0.38).clamp(80.0, 310.0);
        egui::Window::new("Diagnostics")
            .id(egui::Id::new("diagnostics-window"))
            .default_pos(workspace.right_bottom() - egui::vec2(12.0, 12.0))
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .constrain_to(workspace)
            .default_width(290.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .show(ui, |ui| {
                        for diagnostic in &self.diagnostics {
                            let severity = match diagnostic.severity {
                                Severity::Serious => "Serious warning",
                                Severity::Warning => "Warning",
                            };
                            ui.strong(severity);
                            let label = match diagnostic.location {
                                Some(Location::Component(index)) => {
                                    let part = &self.circuit.components[index];
                                    if part.label.is_empty() {
                                        format!("{} #{}", part.kind.name(), index + 1)
                                    } else {
                                        format!(
                                            "{} #{} ({})",
                                            part.kind.name(),
                                            index + 1,
                                            part.label
                                        )
                                    }
                                }
                                Some(Location::Wire(index)) => format!("Wire #{}", index + 1),
                                None => "Circuit".into(),
                            };
                            if let Some(location) = diagnostic.location {
                                if ui.link(label).on_hover_text("Center on canvas").clicked() {
                                    focus = Some(location);
                                }
                            } else {
                                ui.label(label);
                            }
                            ui.label(&diagnostic.message);
                            ui.separator();
                        }
                    });
            });
        if let Some(location) = focus {
            match location {
                Location::Component(index) => {
                    self.view.center_on(self.circuit.components[index].at)
                }
                Location::Wire(index) => {
                    let wire = self.circuit.wires[index];
                    self.view.center_on(Point(
                        (wire.0 .0 + wire.1 .0) / 2,
                        (wire.0 .1 + wire.1 .1) / 2,
                    ));
                }
            }
        }
    }

    fn status(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let cpu = self.cpu.percent.map_or_else(
                    || "Cirtam CPU —".into(),
                    |cpu| format!("Cirtam CPU {cpu:.1}%"),
                );
                ui.monospace(cpu)
                    .on_hover_text("Cirtam process only. 100% equals one logical core.");
                ui.separator();
                let solve = self.last_solve.map_or_else(
                    || "Last DC —".into(),
                    |elapsed| format!("Last DC {:.3} ms", elapsed.as_secs_f64() * 1000.0),
                );
                ui.monospace(solve);
                ui.separator();
                ui.monospace(format!("Zoom {:.0}%", self.view.zoom_percent()));
                ui.separator();
                ui.monospace(if self.circuit != self.saved {
                    "Unsaved"
                } else if self.path.is_some() {
                    "Saved"
                } else {
                    "New circuit"
                });
            });
            if !self.notice.is_empty() {
                ui.label(&self.notice);
            }
        });
    }

    fn confirm(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending else { return };
        let mut proceed = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
            ui.heading("Unsaved circuit");
            ui.horizontal(|ui| {
                if ui.button("Save and continue").clicked() {
                    proceed = self.save(false);
                }
                proceed |= ui.button("Discard changes").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if proceed || cancel {
            self.pending = None;
        }
        if proceed {
            match action {
                FileAction::Close => {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                action => self.execute_file_action(action),
            }
        }
    }
}

impl Editor {
    pub(crate) fn frame(&mut self, ctx: &egui::Context) {
        if !self.allow_close
            && self.saved != self.circuit
            && ctx.input(|input| input.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(FileAction::Close);
        }
        let had_keyboard_focus = self.other_keyboard_focus(ctx);
        self.top(ctx);
        self.cpu.sample();
        ctx.request_repaint_after(crate::telemetry::SAMPLE_INTERVAL);
        self.status(ctx);
        self.sidebar(ctx);
        let workspace = egui::CentralPanel::default()
            .show(ctx, |ui| crate::canvas::show(self, ui))
            .response
            .rect;
        self.diagnostics_window(ctx, workspace);
        self.details_window(ctx);
        if !had_keyboard_focus {
            self.canvas_keys(ctx);
        }
        if self.pending.is_some() {
            self.palette = None;
        }
        let design = self.design();
        if let Some(search) = &mut self.palette {
            match search.show(ctx, design) {
                crate::palette::Outcome::Keep => {}
                crate::palette::Outcome::Close(action) => {
                    self.palette = None;
                    self.view.request_focus = true;
                    if let Some(action) = action {
                        self.activate(action);
                    }
                }
            }
        }
        self.confirm(ctx);
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

fn optional_rating(input: &str, name: &str) -> Result<Option<f64>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    input
        .parse()
        .map(Some)
        .map_err(|_| format!("{name} must be a number or left blank."))
}

fn show_resistor_results(ui: &mut egui::Ui, resistor: &Component, power: f64) {
    let ratings = resistor.ratings;
    let rating = ratings
        .power_watts
        .map(|value| format!("{} W", display_number(value)));
    let tolerance = ratings
        .tolerance_percent
        .map(|value| format!("±{}%", display_number(value)));
    let nominal = format!("{} Ω", display_metric(resistor.value));
    let actual = format!("{} Ω", display_metric(resistor.effective_resistance()));
    for (label, value) in [
        ("Nominal resistance", Some(nominal)),
        ("Tolerance", tolerance),
        ("Actual resistance", Some(actual)),
        ("Wattage limit", rating),
    ] {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.monospace(value.as_deref().unwrap_or("Unspecified"));
        });
    }
    if ratings.power_watts.is_some_and(|rating| power > rating) {
        ui.colored_label(ui.visuals().error_fg_color, "Power rating exceeded");
    }
    if resistor.actual_ohms.is_none() && ratings.tolerance_percent.is_some() {
        ui.label("This circuit has no stored sample for this resistor. The solve uses its nominal resistance.");
    }
}

// The modifiers on the key event describe the moment S was pressed. The
// frame's modifier state may already reflect a subsequently released Shift.
fn save_shortcut(events: &[egui::Event]) -> Option<bool> {
    let mut presses = events.iter().filter_map(|event| match event {
        egui::Event::Key {
            key: Key::S,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } => Some(*modifiers),
        _ => None,
    });
    let modifiers = presses.next()?;
    // Multiple S presses in one frame are ambiguous; never guess a save action.
    if presses.next().is_some() || !modifiers.command || modifiers.alt {
        return None;
    }
    Some(modifiers.shift)
}

fn write_save(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SAVE_NUMBER: AtomicU64 = AtomicU64::new(0);
    let sequence = SAVE_NUMBER.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_file_name(format!(".cirtam-{}-{sequence}.tmp", std::process::id()));
    let mut file = std::fs::File::create_new(&temp)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub fn display_metric(value: f64) -> String {
    let magnitude = value.abs();
    if !(1e-12..1e12).contains(&magnitude) {
        return display_number(value);
    }
    let (scale, prefix) = [
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
    ]
    .into_iter()
    .find(|&(scale, _)| magnitude >= scale)
    .unwrap_or((1.0, ""));
    format!("{}{prefix}", display_number(value / scale))
}

pub fn display_number(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude != 0.0 && !(0.001..100_000.0).contains(&magnitude) {
        format!("{value:.5e}")
    } else {
        format!("{value:.6}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    fn frame(
        editor: &mut Editor,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| editor.frame(ctx),
        )
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn details_cancel_preserves_malformed_data_and_apply_repairs_fixed_components() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        editor.circuit =
            persistence::decode(include_str!("../../../examples/divider.crtm")).unwrap();
        editor.circuit.make_connections_explicit();

        let click_button = |editor: &mut Editor, output: &egui::FullOutput, label: &str| {
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("Details {label} button is visible"));
            for pressed in [true, false] {
                frame(
                    editor,
                    &ctx,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        };

        for kind in [Kind::Ground, Kind::Ammeter, Kind::Voltmeter] {
            let index = editor
                .circuit
                .components
                .iter()
                .position(|part| part.kind == kind)
                .unwrap();
            let component = &mut editor.circuit.components[index];
            component.value = 1.0;
            if kind == Kind::Ground {
                component.actual_ohms = Some(1.0);
            }
            let original = editor.circuit.clone();
            editor.open_details(index, false);
            frame(&mut editor, &ctx, vec![]);
            assert_eq!(editor.circuit, original);

            let undo_count = editor.undo.len();
            editor.details.as_mut().unwrap().label = "Unapplied label".into();
            let output = frame(&mut editor, &ctx, vec![]);
            let shows = |message: &str| {
                output.shapes.iter().any(|shape| {
                    matches!(
                        &shape.shape,
                        egui::Shape::Text(text) if text.galley.job.text.contains(message)
                    )
                })
            };
            assert!(shows("Apply will reset the stored value from 1 to 0."));
            assert_eq!(
                shows("Apply will remove the resistor-only actual resistance"),
                kind == Kind::Ground
            );
            click_button(&mut editor, &output, "Cancel");
            assert!(editor.details.is_none());
            assert_eq!(editor.circuit, original);
            assert_eq!(editor.undo.len(), undo_count);

            editor.open_details(index, false);
            let output = frame(&mut editor, &ctx, vec![]);
            click_button(&mut editor, &output, "Apply");
            assert_eq!(editor.details.as_ref().unwrap().error, None);
            assert_eq!(editor.undo.len(), undo_count + 1);
            let mut expected = original.clone();
            expected.components[index].value = 0.0;
            expected.components[index].actual_ohms = None;
            assert_eq!(editor.circuit, expected);
            solver::solve(&editor.circuit).unwrap();
            let encoded = persistence::encode(&editor.circuit).unwrap();
            assert_eq!(persistence::decode(&encoded).unwrap(), expected);

            editor.history(false);
            assert_eq!(editor.circuit, original);
            editor.history(true);
            assert_eq!(editor.circuit, expected);
        }
    }

    #[test]
    fn details_apply_clears_stray_resistor_fields_from_a_negative_source() {
        let mut editor = Editor::default();
        editor.circuit =
            persistence::decode(include_str!("../../../examples/divider.crtm")).unwrap();
        let source = &mut editor.circuit.components[0];
        source.value = -10.0;
        source.ratings = Ratings {
            power_watts: Some(1.0),
            tolerance_percent: Some(5.0),
        };
        source.actual_ohms = Some(1.0);

        editor.open_details(0, false);
        let mut draft = editor.details.take().unwrap();
        editor.apply_details(&mut draft);
        assert_eq!(draft.error, None);
        let repaired = &editor.circuit.components[0];
        assert_eq!(repaired.value, -10.0);
        assert_eq!(repaired.ratings, Ratings::default());
        assert_eq!(repaired.actual_ohms, None);
        assert_eq!(editor.undo.len(), 1);
        solver::solve(&editor.circuit).unwrap();
        persistence::encode(&editor.circuit).unwrap();
    }

    #[test]
    fn details_apply_preserves_resistor_samples_and_rejects_invalid_retained_samples() {
        let mut editor = Editor::default();
        let mut resistor = Component::new(Kind::Resistor, Point(0, 0));
        resistor.ratings.tolerance_percent = Some(5.0);
        let valid: f64 = 1001.1234567890123;
        resistor.actual_ohms = Some(valid);
        wiring::add_component(&mut editor.circuit, resistor).unwrap();

        editor.open_details(0, false);
        let mut draft = editor.details.take().unwrap();
        editor.apply_details(&mut draft);
        assert!(editor.undo.is_empty());
        draft.label = "Renamed".into();
        editor.apply_details(&mut draft);
        assert_eq!(
            editor.circuit.components[0].actual_ohms.unwrap().to_bits(),
            valid.to_bits()
        );
        draft.power_rating = "0.25".into();
        editor.apply_details(&mut draft);
        assert_eq!(draft.error, None);
        assert_eq!(
            editor.circuit.components[0].actual_ohms.unwrap().to_bits(),
            valid.to_bits()
        );

        editor.circuit.components[0].actual_ohms = None;
        draft.label = "Legacy".into();
        editor.apply_details(&mut draft);
        assert_eq!(draft.error, None);
        assert_eq!(editor.circuit.components[0].actual_ohms, None);
        assert_eq!(editor.circuit.components[0].effective_resistance(), 1000.0);

        editor.history(false);
        editor.circuit.components[0].actual_ohms = Some(2000.0);
        let original = editor.circuit.clone();
        let undo = editor.undo.clone();
        let redo = editor.redo.clone();
        editor.open_details(0, false);
        let mut draft = editor.details.take().unwrap();
        draft.label = "Keep this draft".into();
        editor.apply_details(&mut draft);
        assert!(draft
            .error
            .as_deref()
            .unwrap()
            .contains("Change the nominal resistance or tolerance"));
        assert_eq!(draft.label, "Keep this draft");
        assert_eq!(editor.circuit, original);
        assert_eq!(editor.undo, undo);
        assert_eq!(editor.redo, redo);

        draft.tolerance = "10".into();
        editor.apply_details(&mut draft);
        assert_eq!(draft.error, None);
        assert!((900.0..=1100.0).contains(&editor.circuit.components[0].actual_ohms.unwrap()));
        assert_eq!(editor.circuit.components[0].label, "Keep this draft");
        assert_eq!(editor.undo.len(), undo.len() + 1);
    }

    #[test]
    fn canvas_typing_activates_wire_and_escape_dismisses_search() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        frame(&mut editor, &ctx, vec![]);
        frame(
            &mut editor,
            &ctx,
            vec![key(Key::W), egui::Event::Text("w".into())],
        );
        assert!(editor.palette.is_some());
        frame(
            &mut editor,
            &ctx,
            vec![key(Key::I), egui::Event::Text("i".into())],
        );
        frame(&mut editor, &ctx, vec![key(Key::Enter)]);
        assert!(matches!(editor.tool, Tool::Wire(None)));
        assert!(editor.palette.is_none());

        frame(&mut editor, &ctx, vec![]);
        frame(&mut editor, &ctx, vec![egui::Event::Text("run".into())]);
        assert!(editor.palette.is_some());
        frame(&mut editor, &ctx, vec![key(Key::Escape)]);
        assert!(editor.palette.is_none());
        assert!(matches!(editor.tool, Tool::Wire(None)));
    }

    #[test]
    fn escape_cancels_the_whole_draft_and_completed_wiring_is_undoable() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        wiring::add_component(
            &mut editor.circuit,
            cirtam_core::circuit::Component::new(Kind::Ground, Point(2, 0)),
        )
        .unwrap();
        wiring::add_component(
            &mut editor.circuit,
            cirtam_core::circuit::Component::new(Kind::Ground, Point(8, 0)),
        )
        .unwrap();
        let ids = &editor.circuit.connections.as_ref().unwrap().terminals;
        let start = Target::Node(ids[0][0], Point(0, 0));
        let end = Target::Node(ids[1][0], Point(6, 0));
        editor.tool = Tool::Wire(Some(start));
        editor.route_corners = vec![Point(2, 2), Point(4, 2)];
        frame(&mut editor, &ctx, vec![]);
        frame(&mut editor, &ctx, vec![key(Key::Escape)]);
        assert!(matches!(editor.tool, Tool::Select));
        assert!(editor.route_corners.is_empty());
        assert!(editor.circuit.wires.is_empty());
        assert!(editor.undo.is_empty());

        editor
            .try_change(|c| wiring::connect(c, start, &[Point(2, 2)], end))
            .unwrap();
        let completed = editor.circuit.clone();
        assert_eq!(completed.wires.len(), 2);
        editor.history(false);
        assert!(editor.circuit.wires.is_empty());
        editor.history(true);
        assert_eq!(editor.circuit, completed);
    }

    #[test]
    fn resistance_edits_and_paste_keep_samples_in_history() {
        let mut editor = Editor::default();
        let mut resistor = Component::new(Kind::Resistor, Point(0, 0));
        let ratings = Ratings {
            tolerance_percent: Some(5.0),
            ..Default::default()
        };
        crate::resistance::edit_values(&mut resistor, 10_000.0, ratings).unwrap();
        let first = resistor.actual_ohms.unwrap();
        assert!((9500.0..=10_500.0).contains(&first));
        wiring::add_component(&mut editor.circuit, resistor).unwrap();
        let mut edited = editor.circuit.components[0].clone();
        crate::resistance::edit_values(&mut edited, 20_000.0, ratings).unwrap();
        let second = edited.actual_ohms.unwrap();
        assert!((19_000.0..=21_000.0).contains(&second));
        let updated_rating = Ratings {
            power_watts: Some(0.25),
            ..ratings
        };
        crate::resistance::edit_values(&mut edited, 20_000.0, updated_rating).unwrap();
        assert_eq!(edited.actual_ohms, Some(second));

        editor.change(|c| c.components[0] = edited);
        editor.history(false);
        assert_eq!(editor.circuit.components[0].actual_ohms, Some(first));
        editor.history(true);
        assert_eq!(editor.circuit.components[0].actual_ohms, Some(second));

        // Copy a legacy resistor with tolerance and no sample.
        editor.circuit.components[0].actual_ohms = None;
        editor.selection.insert(Item::Component(0));
        editor.copy(&egui::Context::default());
        editor.paste();
        let pasted = editor.circuit.components[1].actual_ohms.unwrap();
        assert!((19_000.0..=21_000.0).contains(&pasted));
        assert!(editor.circuit.components[0].actual_ohms.is_none());
        editor.history(false);
        editor.history(true);
        assert_eq!(editor.circuit.components[1].actual_ohms, Some(pasted));
        let clear = Ratings {
            tolerance_percent: None,
            ..updated_rating
        };
        crate::resistance::edit_values(&mut editor.circuit.components[1], 20_000.0, clear).unwrap();
        assert!(editor.circuit.components[1].actual_ohms.is_none());
    }

    #[test]
    fn metric_labels_use_case_sensitive_prefixes() {
        assert_eq!(display_metric(10_000.0), "10k");
        assert_eq!(display_metric(2_500_000.0), "2.5M");
        assert_eq!(display_metric(-0.005), "-5m");
        assert_eq!(display_metric(0.0), "0");
    }

    #[test]
    fn branch_removal_preserves_bend_through_undo_and_redo() {
        use cirtam_core::circuit::Wire;
        let mut editor = Editor {
            circuit: Circuit {
                wires: vec![
                    Wire(Point(-3, 0), Point(0, 0)),
                    Wire(Point(0, 0), Point(2, 3)),
                    Wire(Point(0, 0), Point(0, -2)),
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        editor.circuit.make_connections_explicit();
        let original = editor.circuit.clone();
        editor.selection.insert(Item::Wire(2));
        editor.delete();
        let after = editor.circuit.clone();
        assert_eq!(after.wires, original.wires[..2]);
        assert_eq!(after.connections.as_ref().unwrap().bends.len(), 1);
        editor.history(false);
        assert_eq!(editor.circuit, original);
        editor.history(true);
        assert_eq!(editor.circuit, after);
    }

    #[test]
    fn native_clipboard_events_copy_and_paste_circuit_selection() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let mut part = cirtam_core::circuit::Component::new(Kind::Resistor, Point(0, 0));
        part.label = "R1".into();
        part.rotation = 1;
        wiring::add_component(&mut editor.circuit, part).unwrap();
        editor.selection.insert(Item::Component(0));
        frame(&mut editor, &ctx, vec![]);
        frame(&mut editor, &ctx, vec![egui::Event::Copy]);
        frame(
            &mut editor,
            &ctx,
            vec![egui::Event::Paste(CIRCUIT_CLIPBOARD_MARKER.into())],
        );
        assert_eq!(editor.circuit.components[1].label, "R1");
        assert_eq!(editor.circuit.components[1].rotation, 1);
        assert_ne!(
            editor.circuit.components[0].at,
            editor.circuit.components[1].at
        );
        assert_ne!(
            editor.circuit.connections.as_ref().unwrap().terminals[0],
            editor.circuit.connections.as_ref().unwrap().terminals[1]
        );
    }

    #[test]
    fn diagnostics_window_opens_in_lower_right() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        editor.run();
        frame(&mut editor, &ctx, vec![]);
        frame(&mut editor, &ctx, vec![]);
        let bounds = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("diagnostics-window")))
            .expect("missing-ground diagnostic opens the window");
        assert!(
            bounds.center().x > 650.0 && bounds.center().y > 450.0,
            "{bounds:?}"
        );
        assert!(
            bounds.right() <= 1000.0 && bounds.bottom() < 700.0,
            "{bounds:?}"
        );
    }

    #[test]
    fn save_shortcut_uses_keypress_modifiers_and_rejects_ambiguity() {
        let command = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        let shifted = egui::Modifiers {
            shift: true,
            ..command
        };
        let press = |modifiers| egui::Event::Key {
            key: Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };

        // Shift can be released before the frame is processed.
        let released_shift = egui::Event::Key {
            key: Key::S,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: command,
        };
        assert_eq!(save_shortcut(&[press(shifted), released_shift]), Some(true));
        assert_eq!(save_shortcut(&[press(command)]), Some(false));
        assert_eq!(save_shortcut(&[press(egui::Modifiers::NONE)]), None);
        assert_eq!(save_shortcut(&[press(shifted), press(command)]), None);
        assert_eq!(save_shortcut(&[]), None);
    }

    #[test]
    fn modified_wire_shortcut_bypasses_search() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        frame(&mut editor, &ctx, vec![]);
        let modifiers = egui::Modifiers {
            ctrl: true,
            command: true,
            shift: true,
            ..Default::default()
        };
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key: Key::W,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| editor.frame(ctx),
        );
        assert!(matches!(editor.tool, Tool::Wire(None)));
        assert!(editor.palette.is_none());
    }

    #[test]
    fn modified_wire_and_history_shortcuts_use_keypress_modifiers() {
        let command = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        let shifted = egui::Modifiers {
            shift: true,
            ..command
        };
        let press = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };

        for frame_modifiers in [command, shifted, egui::Modifiers::NONE] {
            let ctx = egui::Context::default();
            let mut editor = Editor::default();
            frame(&mut editor, &ctx, vec![]);
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: frame_modifiers,
                    events: vec![press(Key::W, shifted)],
                    ..Default::default()
                },
                |ctx| editor.frame(ctx),
            );
            assert!(matches!(editor.tool, Tool::Wire(None)));
            assert!(editor.palette.is_none());

            editor.tool = Tool::Select;
            editor.change(|circuit| {
                wiring::add_component(circuit, Component::new(Kind::Resistor, Point(0, 0)))
                    .unwrap();
            });
            let changed = editor.circuit.clone();
            editor.history(false);
            assert!(editor.circuit.components.is_empty());
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: frame_modifiers,
                    events: vec![press(Key::Z, shifted)],
                    ..Default::default()
                },
                |ctx| editor.frame(ctx),
            );
            assert_eq!(editor.circuit, changed);
            assert!(editor.redo.is_empty());
            assert_eq!(editor.undo.len(), 1);

            frame(
                &mut editor,
                &ctx,
                vec![egui::Event::Key {
                    key: Key::Z,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: command,
                }],
            );
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: frame_modifiers,
                    events: vec![press(Key::Z, command)],
                    ..Default::default()
                },
                |ctx| editor.frame(ctx),
            );
            assert!(editor.circuit.components.is_empty());
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: frame_modifiers,
                    events: vec![press(Key::Y, command)],
                    ..Default::default()
                },
                |ctx| editor.frame(ctx),
            );
            assert_eq!(editor.circuit, changed);
        }

        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        frame(&mut editor, &ctx, vec![]);
        let alt_shifted = egui::Modifiers {
            alt: true,
            ..shifted
        };
        let _ = ctx.run(
            egui::RawInput {
                modifiers: shifted,
                events: vec![
                    press(Key::W, alt_shifted),
                    press(Key::Z, egui::Modifiers::NONE),
                ],
                ..Default::default()
            },
            |ctx| editor.frame(ctx),
        );
        assert!(matches!(editor.tool, Tool::Select));
        assert!(editor.undo.is_empty() && editor.redo.is_empty());
    }

    #[test]
    fn rotate_shortcut_changes_preview_and_escape_cancels_placement() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        editor.place(Kind::Voltage);
        frame(&mut editor, &ctx, vec![]);
        frame(
            &mut editor,
            &ctx,
            vec![key(Key::R), egui::Event::Text("r".into())],
        );
        assert!(matches!(
            editor.tool,
            Tool::Place {
                kind: Kind::Voltage,
                rotation: 1
            }
        ));
        assert!(editor.palette.is_none());
        assert!(editor.circuit.components.is_empty());
        frame(&mut editor, &ctx, vec![key(Key::Escape)]);
        assert!(matches!(editor.tool, Tool::Select));
    }
}
