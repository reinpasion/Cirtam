use crate::simulation::{Mode, Settings, Snapshot};
use cirtam_core::circuit::Circuit;
use eframe::egui::{self, pos2, vec2, Align2, FontId, Pos2, Stroke, StrokeKind};

pub fn settings(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut settings.mode, Mode::Timed, "Timed");
        ui.selectable_value(&mut settings.mode, Mode::Live, "Live");
        ui.label("Timestep (s)");
        ui.add(
            egui::DragValue::new(&mut settings.timestep)
                .speed(0.01)
                .max_decimals(12),
        );
        match settings.mode {
            Mode::Timed => {
                ui.label("End time (s)");
                ui.add(
                    egui::DragValue::new(&mut settings.end)
                        .speed(0.1)
                        .max_decimals(12),
                );
                ui.label("Save results from (s)");
                ui.add(
                    egui::DragValue::new(&mut settings.save_from)
                        .speed(0.1)
                        .max_decimals(12),
                );
                ui.label("A final step may be shorter to reach the end time.");
            }
            Mode::Live => {
                ui.checkbox(&mut settings.unlimited, "As fast as possible");
                ui.add_enabled_ui(!settings.unlimited, |ui| {
                    ui.label("Target speed (simulated s per real s)");
                    ui.add(egui::DragValue::new(&mut settings.speed).speed(0.1));
                });
            }
        }
    });
}

fn step_points(samples: impl Iterator<Item = Pos2>) -> Vec<Pos2> {
    let mut points: Vec<Pos2> = Vec::new();
    for sample in samples {
        if let Some(previous) = points.last() {
            if previous.y != sample.y {
                points.push(pos2(sample.x, previous.y));
            }
        }
        points.push(sample);
    }
    points
}

#[derive(Default)]
pub struct TraceSelection {
    component: usize,
    current: bool,
}

impl TraceSelection {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        workspace: egui::Rect,
        circuit: &Circuit,
        snapshot: &Snapshot,
    ) {
        if circuit.components.is_empty() {
            return;
        }
        self.component = self.component.min(circuit.components.len() - 1);
        let name = |index: usize| {
            let component = &circuit.components[index];
            if component.label.is_empty() {
                format!("{} #{}", component.kind.name(), index + 1)
            } else {
                component.label.clone()
            }
        };
        egui::Window::new("DC operating-point samples")
            .id(egui::Id::new("dc-samples"))
            .default_pos(workspace.left_bottom() + vec2(12.0, -12.0))
            .pivot(Align2::LEFT_BOTTOM)
            .constrain_to(workspace)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("trace-component")
                        .selected_text(name(self.component))
                        .show_ui(ui, |ui| {
                            for index in 0..circuit.components.len() {
                                ui.selectable_value(&mut self.component, index, name(index));
                            }
                        });
                    ui.selectable_value(&mut self.current, false, "Voltage");
                    ui.selectable_value(&mut self.current, true, "Current");
                });
                let values: Vec<_> = snapshot
                    .history
                    .iter()
                    .filter_map(|sample| {
                        let reading = sample.solution.readings.get(self.component)?;
                        let value = if self.current {
                            reading.current?
                        } else {
                            reading.voltage
                        };
                        Some((sample.time, value))
                    })
                    .collect();
                let Some(&(start, first)) = values.first() else {
                    ui.label("No retained samples for this reading.");
                    return;
                };
                let end = values.last().expect("nonempty trace").0;
                let (min, max) = values
                    .iter()
                    .fold((first, first), |(min, max), &(_, value)| {
                        (min.min(value), max.max(value))
                    });
                let unit = if self.current { "A" } else { "V" };
                ui.monospace(format!(
                    "{}–{} {unit}",
                    crate::app::display_metric(min),
                    crate::app::display_metric(max)
                ));
                let (response, painter) = ui.allocate_painter(
                    vec2(ui.available_width().max(10.0), 130.0),
                    egui::Sense::hover(),
                );
                let rect = response.rect.shrink(3.0);
                let scale = |value: f64, from: f64, to: f64| {
                    if from == to {
                        0.5
                    } else {
                        // Scaling first keeps subtraction finite for opposite extreme values.
                        let magnitude = from.abs().max(to.abs());
                        ((value / magnitude - from / magnitude)
                            / (to / magnitude - from / magnitude)) as f32
                    }
                };
                let points = step_points(values.iter().map(|&(time, value)| {
                    pos2(
                        egui::lerp(rect.x_range(), scale(time, start, end)),
                        egui::lerp(rect.bottom()..=rect.top(), scale(value, min, max)),
                    )
                }));
                let color = ui.visuals().selection.stroke.color;
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    StrokeKind::Inside,
                );
                if points.len() == 1 {
                    painter.circle_filled(points[0], 2.5, color);
                } else {
                    painter.add(egui::Shape::line(points, Stroke::new(1.6_f32, color)));
                }
                let font = FontId::monospace(11.0);
                painter.text(
                    rect.left_bottom() + vec2(4.0, -2.0),
                    Align2::LEFT_BOTTOM,
                    format!("{start:.6} s"),
                    font.clone(),
                    ui.visuals().text_color(),
                );
                painter.text(
                    rect.right_bottom() + vec2(-4.0, -2.0),
                    Align2::RIGHT_BOTTOM,
                    format!("{end:.6} s"),
                    font,
                    ui.visuals().text_color(),
                );
                let first_step = snapshot.history.front().expect("nonempty trace").step;
                let last_step = snapshot.history.back().expect("nonempty trace").step;
                ui.label(format!(
                    "Retained steps {first_step}–{last_step} ({} samples)",
                    snapshot.history.len()
                ));
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_holds_previous_value_until_the_next_sample() {
        let samples = [pos2(1.0, 8.0), pos2(2.0, 8.0), pos2(4.0, 3.0)];
        assert_eq!(
            step_points(samples.into_iter()),
            [samples[0], samples[1], pos2(4.0, 8.0), samples[2]]
        );
        assert_eq!(step_points(samples[..1].iter().copied()), [samples[0]]);
    }
}
