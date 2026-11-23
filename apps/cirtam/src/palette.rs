use cirtam_core::circuit::Kind;
use eframe::egui::{self, Key, Modifiers};

#[derive(Clone, Copy)]
pub enum Action {
    Select,
    Wire,
    Run,
    Stop,
    Place(Kind),
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Wire => "Wire",
            Self::Run => "Run",
            Self::Stop => "Stop",
            Self::Place(kind) => kind.name(),
        }
    }

    fn matches(self, query: &str) -> bool {
        self.label().to_ascii_lowercase().contains(query)
            || matches!(self, Self::Place(Kind::Ground)) && "gnd".contains(query)
    }
}

fn suggestions(query: &str, design: bool) -> Vec<Action> {
    let query = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let mut actions = if design {
        [Action::Select, Action::Wire, Action::Run]
            .into_iter()
            .chain(Kind::ALL.into_iter().map(Action::Place))
            .collect()
    } else {
        vec![Action::Stop]
    };
    actions.retain(|action| action.matches(&query));
    actions.sort_by_key(|action| !action.label().to_ascii_lowercase().starts_with(&query));
    actions
}

pub enum Outcome {
    Keep,
    Close(Option<Action>),
}

pub struct Search {
    query: String,
    selected: usize,
    initial_focus: bool,
}

impl Search {
    pub fn new(query: String) -> Self {
        Self {
            query,
            selected: 0,
            initial_focus: true,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, design: bool) -> Outcome {
        let input_id = egui::Id::new("canvas-search-input");
        let (escape, enter, up, down) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::Escape),
                input.consume_key(Modifiers::NONE, Key::Enter),
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
            )
        });
        let modal = egui::Modal::new(egui::Id::new("canvas-search")).show(ctx, |ui| {
            ui.set_min_width(270.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(input_id)
                    .hint_text("Search commands and components")
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut self.initial_focus) {
                field.request_focus();
                if let Some(mut state) = egui::TextEdit::load_state(ctx, input_id) {
                    use egui::text::{CCursor, CCursorRange};
                    state
                        .cursor
                        .set_char_range(Some(CCursorRange::one(CCursor::new(
                            self.query.chars().count(),
                        ))));
                    state.store(ctx, input_id);
                }
            }
            if field.changed() {
                self.selected = 0;
            }
            let actions = suggestions(&self.query, design);
            let count = actions.len();
            if count == 0 {
                ui.label("No matches");
                return None;
            }
            if down {
                self.selected = (self.selected + 1) % count;
            }
            if up {
                self.selected = (self.selected + count - 1) % count;
            }
            let mut chosen = enter.then(|| actions[self.selected]);
            for (index, action) in actions.into_iter().enumerate() {
                if ui
                    .selectable_label(index == self.selected, action.label())
                    .clicked()
                {
                    chosen = Some(action);
                }
            }
            chosen
        });
        let outcome = if escape || modal.backdrop_response.clicked() {
            Outcome::Close(None)
        } else if let Some(action) = modal.inner {
            Outcome::Close(Some(action))
        } else {
            Outcome::Keep
        };
        if matches!(outcome, Outcome::Close(_)) {
            ctx.memory_mut(|memory| memory.surrender_focus(input_id));
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_filters_to_supported_actions_for_current_mode() {
        assert!(matches!(suggestions("wi", true).as_slice(), [Action::Wire]));
        assert!(matches!(
            suggestions("gnd", true).as_slice(),
            [Action::Place(Kind::Ground)]
        ));
        assert!(suggestions("capacitor", true).is_empty());
        assert!(matches!(suggestions("", false).as_slice(), [Action::Stop]));
    }
}
