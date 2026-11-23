use crate::circuit::{Circuit, Kind, Point};
use crate::diagnostics::{Diagnostic, Location, Severity};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub voltage: f64,
    pub current: Option<f64>,
    pub resistor_power: Option<f64>,
}

#[derive(Debug)]
pub struct Solution {
    pub readings: Vec<Reading>,
    pub diagnostics: Vec<Diagnostic>,
}

struct Sets(Vec<usize>);

impl Sets {
    fn new(size: usize) -> Self {
        Self((0..size).collect())
    }

    fn root(&mut self, mut item: usize) -> usize {
        while self.0[item] != item {
            self.0[item] = self.0[self.0[item]];
            item = self.0[item];
        }
        item
    }

    fn join(&mut self, a: usize, b: usize) {
        let a = self.root(a);
        let b = self.root(b);
        self.0[b] = a;
    }
}

pub fn solve(circuit: &Circuit) -> Result<Solution, Vec<Diagnostic>> {
    circuit
        .validate_geometry()
        .map_err(|error| vec![Diagnostic::serious(None, error)])?;
    let mut diagnostics = Vec::new();
    for (index, component) in circuit.components.iter().enumerate() {
        if let Err(error) = component.validate_values() {
            diagnostics.push(Diagnostic::serious(Some(Location::Component(index)), error));
        }
    }
    if circuit.components.len() > 512 || circuit.wires.len() > 2048 {
        diagnostics.push(Diagnostic::serious(
            None,
            "This version supports up to 512 components and 2048 wire segments per solve.",
        ));
        return Err(diagnostics);
    }

    let (mut nets, terminal_ids, wire_ids, reference) = electrical_nodes(circuit);
    // Count physical connections before merging distinct ground symbols into one reference.
    let mut terminal_counts = HashMap::<usize, usize>::new();
    for &id in terminal_ids.iter().flatten() {
        *terminal_counts.entry(nets.root(id)).or_default() += 1;
    }
    for (index, ids) in terminal_ids.iter().enumerate() {
        for (terminal, &id) in ids.iter().enumerate() {
            if terminal_counts[&nets.root(id)] == 1 {
                diagnostics.push(Diagnostic::warning(
                    Location::Component(index),
                    format!(
                        "Terminal {} has no connection to another terminal.",
                        terminal + 1
                    ),
                ));
            }
        }
    }

    let mut has_ground = false;
    for (component, ids) in circuit.components.iter().zip(&terminal_ids) {
        if component.kind == Kind::Ground {
            has_ground = true;
            nets.join(reference, ids[0]);
        }
    }
    if !has_ground {
        diagnostics.push(Diagnostic::serious(
            None,
            "Missing ground. Add and connect a ground reference.",
        ));
    }

    let ground = nets.root(reference);
    let mut nodes = HashMap::from([(ground, 0)]);
    let mut terminals = Vec::with_capacity(circuit.components.len());
    for ids in &terminal_ids {
        let mut pair = [0; 2];
        for (i, &id) in ids.iter().enumerate() {
            let next = nodes.len();
            pair[i] = *nodes.entry(nets.root(id)).or_insert(next);
        }
        terminals.push(pair);
    }
    for (index, &[start, _]) in wire_ids.iter().enumerate() {
        if !nodes.contains_key(&nets.root(start)) {
            diagnostics.push(Diagnostic::serious(
                Some(Location::Wire(index)),
                "This disconnected wire network does not reach any component terminal.",
            ));
        }
    }

    let mut paths = Sets::new(nodes.len());
    let mut ideal_branches = Sets::new(nodes.len());
    let mut branch_count = 0;
    for (index, (c, &[a, b])) in circuit.components.iter().zip(&terminals).enumerate() {
        let location = Location::Component(index);
        match c.kind {
            Kind::Resistor if a == b => diagnostics.push(Diagnostic::warning(
                location,
                "Both resistor terminals are on the same net. The resistor is bypassed.",
            )),
            Kind::Resistor => paths.join(a, b),
            Kind::Voltage | Kind::Ammeter => {
                if a == b {
                    diagnostics.push(Diagnostic::serious(
                        Some(location),
                        format!(
                            "{} is shorted. Both terminals are on the same net.",
                            c.kind.name()
                        ),
                    ));
                    continue;
                }
                if ideal_branches.root(a) == ideal_branches.root(b) {
                    diagnostics.push(Diagnostic::serious(Some(location),
                        "This branch closes a loop of ideal voltage sources or ammeters. The loop has inconsistent voltages or an undetermined branch current."));
                    continue;
                }
                ideal_branches.join(a, b);
                paths.join(a, b);
                branch_count += 1;
            }
            _ => {}
        }
    }
    if has_ground {
        let reference_path = paths.root(0);
        for (index, (component, nodes)) in circuit.components.iter().zip(&terminals).enumerate() {
            for (terminal, _) in component.terminals().enumerate() {
                if paths.root(nodes[terminal]) != reference_path {
                    diagnostics.push(Diagnostic::serious(Some(Location::Component(index)),
                        format!("Terminal {} is floating. It needs a DC path to ground through wires, resistors, voltage sources, or ammeters.", terminal + 1)));
                }
            }
        }
    }
    if has_ground {
        let reference_path = paths.root(0);
        for (index, &[start, _]) in wire_ids.iter().enumerate() {
            if let Some(&node) = nodes.get(&nets.root(start)) {
                if paths.root(node) != reference_path {
                    diagnostics.push(Diagnostic::serious(
                        Some(Location::Wire(index)),
                        "This wire is on a floating net with no DC path to ground.",
                    ));
                }
            }
        }
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Serious)
    {
        return Err(diagnostics);
    }

    let node_count = nodes.len() - 1;
    let size = node_count + branch_count;
    let mut matrix = vec![vec![0.0; size + 1]; size];
    let mut branch = node_count;
    for (c, &[a, b]) in circuit.components.iter().zip(&terminals) {
        let a = a.checked_sub(1);
        let b = b.checked_sub(1);
        match c.kind {
            Kind::Resistor if a != b => {
                let conductance = 1.0 / c.effective_resistance();
                for (i, sign_i) in [(a, 1.0), (b, -1.0)] {
                    for (j, sign_j) in [(a, 1.0), (b, -1.0)] {
                        if let (Some(i), Some(j)) = (i, j) {
                            matrix[i][j] += sign_i * sign_j * conductance;
                        }
                    }
                }
            }
            Kind::Current => {
                for (node, sign) in [(a, -1.0), (b, 1.0)] {
                    if let Some(i) = node {
                        matrix[i][size] += sign * c.value;
                    }
                }
            }
            Kind::Voltage | Kind::Ammeter => {
                for (node, sign) in [(a, 1.0), (b, -1.0)] {
                    if let Some(i) = node {
                        matrix[i][branch] += sign;
                        matrix[branch][i] += sign;
                    }
                }
                matrix[branch][size] = if c.kind == Kind::Voltage {
                    c.value
                } else {
                    0.0
                };
                branch += 1;
            }
            _ => {}
        }
    }

    let solved = match eliminate(matrix) {
        Ok(solved) => solved,
        Err(error) => {
            diagnostics.push(Diagnostic::serious(None, error));
            return Err(diagnostics);
        }
    };
    let voltage_at = |node: usize| node.checked_sub(1).map_or(0.0, |i| solved[i]);
    let mut branch = node_count;
    let mut readings = Vec::with_capacity(terminals.len());
    for (index, (c, [a, b])) in circuit.components.iter().zip(terminals).enumerate() {
        let voltage = voltage_at(a) - voltage_at(b);
        let current = match c.kind {
            Kind::Resistor => Some(voltage / c.effective_resistance()),
            Kind::Current => Some(c.value),
            Kind::Voltmeter => Some(0.0),
            Kind::Ground => None,
            Kind::Voltage | Kind::Ammeter => {
                branch += 1;
                Some(solved[branch - 1])
            }
        };
        let resistor_power =
            (c.kind == Kind::Resistor).then(|| voltage * (voltage / c.effective_resistance()));
        if !voltage.is_finite()
            || current.is_some_and(|i| !i.is_finite())
            || resistor_power.is_some_and(|power| !power.is_finite())
        {
            diagnostics.push(Diagnostic::serious(
                Some(Location::Component(index)),
                "The calculated readings exceed the numeric range.",
            ));
            return Err(diagnostics);
        }
        if let (Some(power), Some(rating)) = (resistor_power, c.ratings.power_watts) {
            if power > rating {
                diagnostics.push(Diagnostic::serious(
                    Some(Location::Component(index)),
                    format!("DC dissipation of {power:.6} W exceeds the {rating} W power rating."),
                ));
            }
        }
        readings.push(Reading {
            voltage,
            current,
            resistor_power,
        });
    }
    Ok(Solution {
        readings,
        diagnostics,
    })
}

fn eliminate(mut matrix: Vec<Vec<f64>>) -> Result<Vec<f64>, String> {
    let size = matrix.len();
    let failure = || "The circuit equations are singular or numerically out of range.".to_owned();
    for row in &mut matrix {
        let scale = row[..size].iter().fold(0.0_f64, |max, x| max.max(x.abs()));
        if scale == 0.0 || !scale.is_finite() {
            return Err(failure());
        }
        for x in row {
            *x /= scale;
        }
    }
    for col in 0..size {
        let pivot = (col..size)
            .max_by(|&a, &b| matrix[a][col].abs().total_cmp(&matrix[b][col].abs()))
            .unwrap();
        matrix.swap(col, pivot);
        let divisor = matrix[col][col];
        if !divisor.is_finite() || divisor.abs() < 1e-13 {
            return Err(failure());
        }
        let (finished, remaining) = matrix.split_at_mut(col + 1);
        let pivot_row = &finished[col];
        for row in remaining {
            let factor = row[col] / divisor;
            for j in col..=size {
                row[j] -= factor * pivot_row[j];
            }
        }
    }
    let mut results = vec![0.0; size];
    for i in (0..size).rev() {
        let rest: f64 = (i + 1..size).map(|j| results[j] * matrix[i][j]).sum();
        results[i] = (matrix[i][size] - rest) / matrix[i][i];
        if !results[i].is_finite() {
            return Err(failure());
        }
    }
    Ok(results)
}

fn electrical_nodes(circuit: &Circuit) -> (Sets, Vec<Vec<usize>>, Vec<[usize; 2]>, usize) {
    if let Some(links) = &circuit.connections {
        let mut indices = HashMap::new();
        let mut index = |node| {
            let next = indices.len();
            *indices.entry(node).or_insert(next)
        };
        let terminals = links
            .terminals
            .iter()
            .map(|ids| ids.iter().copied().map(&mut index).collect())
            .collect();
        let wires: Vec<_> = links
            .wires
            .iter()
            .map(|&[a, b]| [index(a), index(b)])
            .collect();
        let reference = indices.len();
        let mut nets = Sets::new(reference + 1);
        for &[a, b] in &wires {
            nets.join(a, b);
        }
        return (nets, terminals, wires, reference);
    }

    // Legacy documents join coordinates and endpoints lying on wire interiors.
    let mut points = HashMap::<Point, usize>::new();
    let mut index = |point| {
        let next = points.len();
        *points.entry(point).or_insert(next)
    };
    let terminals = circuit
        .components
        .iter()
        .map(|c| c.terminals().map(&mut index).collect())
        .collect();
    let wires: Vec<_> = circuit
        .wires
        .iter()
        .map(|wire| [index(wire.0), index(wire.1)])
        .collect();
    let reference = points.len();
    let mut nets = Sets::new(reference + 1);
    for (wire, &[start, _]) in circuit.wires.iter().zip(&wires) {
        for (&point, &id) in &points {
            if wire.contains(point) {
                nets.join(start, id);
            }
        }
    }
    (nets, terminals, wires, reference)
}
