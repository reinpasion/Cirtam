use crate::app::Item;
use cirtam_core::{
    circuit::{Circuit, Component, Point, Wire},
    topology::Node,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Node(Node, Point),
    Interior(usize, Point),
}

impl Target {
    pub fn point(self) -> Point {
        match self {
            Self::Node(_, point) | Self::Interior(_, point) => point,
        }
    }
}

pub fn add_component(circuit: &mut Circuit, component: Component) -> Result<(), String> {
    let links = circuit.connections.as_mut().unwrap();
    let mut ids = links.node_allocator();
    let terminals = component
        .terminals()
        .map(|_| ids.allocate())
        .collect::<Result<_, _>>()?;
    links.terminals.push(terminals);
    circuit.components.push(component);
    Ok(())
}

fn add_wire(circuit: &mut Circuit, wire: Wire, ids: [Node; 2]) {
    circuit.wires.push(wire);
    circuit.connections.as_mut().unwrap().wires.push(ids);
}

pub fn connect(
    circuit: &mut Circuit,
    start: Target,
    corners: &[Point],
    end: Target,
) -> Result<(), String> {
    let mut allocator = None;
    let mut fresh = || {
        allocator
            .get_or_insert_with(|| circuit.connections.as_ref().unwrap().node_allocator())
            .allocate()
    };
    let mut target_id = |target| match target {
        Target::Node(id, _) => Ok(id),
        Target::Interior(..) => fresh(),
    };
    let start_id = target_id(start)?;
    let end_id = target_id(end)?;
    let corner_ids = corners
        .iter()
        .map(|_| fresh())
        .collect::<Result<Vec<_>, _>>()?;

    attach(circuit, start, start_id);
    let end = match (start, end) {
        (Target::Interior(a, _), Target::Interior(b, point))
            if a == b && !circuit.wires[b].contains(point) =>
        {
            Target::Interior(circuit.wires.len() - 1, point)
        }
        _ => end,
    };
    attach(circuit, end, end_id);
    if start.point() == end.point() && corners.is_empty() {
        let links = circuit.connections.as_mut().unwrap();
        for id in links
            .terminals
            .iter_mut()
            .flatten()
            .chain(links.wires.iter_mut().flatten())
        {
            if *id == end_id {
                *id = start_id;
            }
        }
        return Ok(());
    }
    circuit
        .connections
        .as_mut()
        .unwrap()
        .bends
        .extend(corner_ids.iter().copied());
    let mut previous = (start_id, start.point());
    for (node, point) in corners
        .iter()
        .copied()
        .zip(corner_ids)
        .map(|(p, n)| (n, p))
        .chain([(end_id, end.point())])
    {
        add_wire(circuit, Wire(previous.1, point), [previous.0, node]);
        previous = (node, point);
    }
    if corners.is_empty() && matches!((start, end), (Target::Node(..), Target::Node(..))) {
        simplify(circuit, start_id);
        simplify(circuit, end_id);
    }
    Ok(())
}

fn attach(circuit: &mut Circuit, target: Target, node: Node) {
    match target {
        Target::Node(_, _) => {
            circuit.connections.as_mut().unwrap().bends.remove(&node);
        }
        Target::Interior(index, point) => {
            let links = circuit.connections.as_mut().unwrap();
            let Wire(a, b) = circuit.wires[index];
            let [start, end] = links.wires[index];
            circuit.wires[index] = Wire(a, point);
            links.wires[index] = [start, node];
            add_wire(circuit, Wire(point, b), [node, end]);
        }
    }
}

fn disconnect(circuit: &mut Circuit, nodes: &HashSet<Node>) -> Result<(), String> {
    if nodes.is_empty() {
        return Ok(());
    }
    let links = circuit.connections.as_mut().unwrap();
    let mut allocator = links.node_allocator();
    let fresh = links
        .terminals
        .iter()
        .flatten()
        .chain(links.wires.iter().flatten())
        .filter(|id| nodes.contains(id))
        .map(|_| allocator.allocate())
        .collect::<Result<Vec<_>, _>>()?;
    links.bends.retain(|node| !nodes.contains(node));
    for (id, fresh) in links
        .terminals
        .iter_mut()
        .flatten()
        .chain(links.wires.iter_mut().flatten())
        .filter(|id| nodes.contains(*id))
        .zip(fresh)
    {
        *id = fresh;
    }
    Ok(())
}

pub fn counts(circuit: &Circuit) -> HashMap<Node, (Point, usize)> {
    let mut counts = HashMap::new();
    if let Some(links) = &circuit.connections {
        for (node, point) in links.endpoints(circuit) {
            counts.entry(node).or_insert((point, 0)).1 += 1;
        }
    }
    counts
}

/// Wire segments joined by invisible bends form one selectable wire path.
pub fn wire_path(circuit: &Circuit, index: usize) -> HashSet<usize> {
    let links = circuit.connections.as_ref().unwrap();
    let mut path = HashSet::from([index]);
    let mut pending = vec![index];
    while let Some(segment) = pending.pop() {
        for node in links.wires[segment] {
            if links.bends.contains(&node) {
                for (other, endpoints) in links.wires.iter().enumerate() {
                    if endpoints.contains(&node) && path.insert(other) {
                        pending.push(other);
                    }
                }
            }
        }
    }
    path
}

pub fn selected_segments(circuit: &Circuit, selection: &HashSet<Item>) -> HashSet<usize> {
    let mut segments = HashSet::new();
    for &item in selection {
        if let Item::Wire(index) = item {
            if !segments.contains(&index) {
                segments.extend(wire_path(circuit, index));
            }
        }
    }
    segments
}

pub fn simplify(circuit: &mut Circuit, node: Node) -> bool {
    let links = circuit.connections.as_mut().unwrap();
    if links.terminals.iter().flatten().any(|&id| id == node)
        || links
            .wires
            .iter()
            .flatten()
            .filter(|&&id| id == node)
            .count()
            != 2
    {
        return false;
    }
    links.bends.insert(node)
}

pub fn delete(circuit: &mut Circuit, selection: &HashSet<Item>) -> Result<(), String> {
    let before = counts(circuit);
    let segments = selected_segments(circuit, selection);
    let links = circuit.connections.as_mut().unwrap();
    retain_pairs(&mut circuit.components, &mut links.terminals, |i| {
        !selection.contains(&Item::Component(i))
    });
    retain_pairs(&mut circuit.wires, &mut links.wires, |i| {
        !segments.contains(&i)
    });
    let surviving_nodes: HashSet<_> = links.wires.iter().flatten().copied().collect();
    links.bends.retain(|node| surviving_nodes.contains(node));
    let mut candidates: Vec<_> = counts(circuit)
        .into_iter()
        .filter_map(|(node, (_, count))| (count == 2 && before[&node].1 > 2).then_some(node))
        .collect();
    candidates.sort_unstable();
    for node in candidates {
        if !selection.contains(&Item::Node(node)) {
            simplify(circuit, node);
        }
    }
    let nodes = selection
        .iter()
        .filter_map(|item| match *item {
            Item::Node(node) => Some(node),
            _ => None,
        })
        .collect();
    disconnect(circuit, &nodes)
}

fn retain_pairs<T, U>(values: &mut Vec<T>, ids: &mut Vec<U>, keep: impl Fn(usize) -> bool) {
    let mut index = 0;
    values.retain(|_| {
        let retain = keep(index);
        index += 1;
        retain
    });
    index = 0;
    ids.retain(|_| {
        let retain = keep(index);
        index += 1;
        retain
    });
}

/// Reconcile changed terminal positions with the wire geometry. If a node has
/// stationary terminals and must also move, a wire bridges the two positions.
pub fn follow_nodes(
    before: &Circuit,
    after: &mut Circuit,
    shifts: &HashMap<Node, Point>,
) -> Result<(), String> {
    let links = after.connections.as_mut().unwrap();
    let mut terminal_positions: HashMap<Node, Vec<Point>> = HashMap::new();
    for (part, ids) in after.components.iter().zip(&links.terminals) {
        for (point, &node) in part.terminals().zip(ids) {
            terminal_positions.entry(node).or_default().push(point);
        }
    }
    let mut wire_degree = HashMap::<Node, usize>::new();
    for &node in links.wires.iter().flatten() {
        *wire_degree.entry(node).or_default() += 1;
    }
    let old_positions = counts(before);
    let mut destinations = HashMap::new();
    for (&node, &(old, _)) in &old_positions {
        let positions = terminal_positions.get(&node).map_or(&[][..], Vec::as_slice);
        let destination = shifts.get(&node).copied().unwrap_or_else(|| {
            if positions.contains(&old) || wire_degree.get(&node).copied().unwrap_or(0) > 1 {
                old
            } else {
                positions.first().copied().unwrap_or(old)
            }
        });
        destinations.insert(node, destination);
    }
    let mut allocator = None;
    let mut splits = HashMap::new();
    let mut bridges = Vec::new();
    for (part, ids) in after.components.iter().zip(&links.terminals) {
        for (point, &node) in part.terminals().zip(ids) {
            let destination = destinations[&node];
            if point != destination {
                if let std::collections::hash_map::Entry::Vacant(split) =
                    splits.entry((node, point))
                {
                    let id = allocator
                        .get_or_insert_with(|| links.node_allocator())
                        .allocate()?;
                    split.insert(id);
                    bridges.push((Wire(destination, point), [node, id]));
                }
            }
        }
    }
    for (wire, &[a, b]) in after.wires.iter_mut().zip(&links.wires) {
        *wire = Wire(destinations[&a], destinations[&b]);
    }
    for (part, ids) in after.components.iter().zip(&mut links.terminals) {
        for (point, node) in part.terminals().zip(ids) {
            if let Some(&id) = splits.get(&(*node, point)) {
                *node = id;
            }
        }
    }
    for (wire, ids) in bridges {
        after.wires.push(wire);
        links.wires.push(ids);
    }
    Ok(())
}

pub fn translated(
    circuit: &Circuit,
    selection: &HashSet<Item>,
    delta: Point,
) -> Result<Circuit, String> {
    let mut moved = circuit.clone();
    let links = circuit.connections.as_ref().unwrap();
    let positions = counts(circuit);
    let mut shifts = HashMap::new();
    for &item in selection {
        match item {
            Item::Component(index) => {
                moved.components[index].at = moved.components[index].at.offset(delta)
            }
            Item::Node(node) => {
                shifts.insert(node, positions[&node].0.offset(delta));
            }
            Item::Wire(_) => {}
        }
    }
    for index in selected_segments(circuit, selection) {
        for node in links.wires[index] {
            shifts.insert(node, positions[&node].0.offset(delta));
        }
    }
    follow_nodes(circuit, &mut moved, &shifts)?;
    Ok(moved)
}

pub fn copy(circuit: &Circuit, selection: &HashSet<Item>) -> Circuit {
    let links = circuit.connections.as_ref().unwrap();
    let mut copy = Circuit {
        connections: Some(Default::default()),
        ..Default::default()
    };
    let selected = copy.connections.as_mut().unwrap();
    let segments = selected_segments(circuit, selection);
    for (index, part) in circuit.components.iter().enumerate() {
        if selection.contains(&Item::Component(index)) {
            copy.components.push(part.clone());
            selected.terminals.push(links.terminals[index].clone());
        }
    }
    for (index, &wire) in circuit.wires.iter().enumerate() {
        if segments.contains(&index) {
            copy.wires.push(wire);
            selected.wires.push(links.wires[index]);
        }
    }
    selected.bends.extend(
        selected
            .wires
            .iter()
            .flatten()
            .filter(|node| links.bends.contains(node))
            .copied(),
    );
    copy
}

pub fn paste(circuit: &mut Circuit, copy: &Circuit) -> Result<HashSet<Item>, String> {
    let bounds = |c: &Circuit| {
        c.components
            .iter()
            .map(|p| p.at)
            .chain(
                c.connections
                    .as_ref()
                    .unwrap()
                    .endpoints(c)
                    .map(|(_, point)| point),
            )
            .fold(None, |bounds: Option<[i32; 4]>, p| {
                let [left, top, right, bottom] = bounds.unwrap_or([p.0, p.1, p.0, p.1]);
                Some([left.min(p.0), top.min(p.1), right.max(p.0), bottom.max(p.1)])
            })
    };
    let [x, y, right, bottom] = bounds(copy).ok_or("Copy components or wires first.")?;
    let shifts = if let Some([a, b, c, d]) = bounds(circuit) {
        [
            Point(c - x + 4, 0),
            Point(a - right - 4, 0),
            Point(0, d - y + 4),
            Point(0, b - bottom - 4),
        ]
    } else {
        [Point(0, 0); 4]
    };
    let delta = shifts
        .into_iter()
        .find(|&delta| {
            [Point(x, y), Point(right, bottom)]
                .into_iter()
                .map(|p| p.offset(delta))
                .all(|p| (-99_998..=99_998).contains(&p.0) && (-99_998..=99_998).contains(&p.1))
        })
        .ok_or("The copied objects do not fit on the grid.")?;

    let links = circuit.connections.as_mut().unwrap();
    let mut allocator = links.node_allocator();
    let mut remapped = HashMap::new();
    let source = copy.connections.as_ref().unwrap();
    for &old in source
        .terminals
        .iter()
        .flatten()
        .chain(source.wires.iter().flatten())
        .chain(&source.bends)
    {
        if let std::collections::hash_map::Entry::Vacant(entry) = remapped.entry(old) {
            entry.insert(allocator.allocate()?);
        }
    }
    let id = |old| remapped[&old];
    let mut selection = HashSet::new();
    for (part, ids) in copy.components.iter().zip(&source.terminals) {
        let mut part = part.clone();
        part.at = part.at.offset(delta);
        selection.insert(Item::Component(circuit.components.len()));
        circuit.components.push(part);
        links.terminals.push(ids.iter().copied().map(id).collect());
    }
    let mut visited = HashSet::new();
    for (index, (&Wire(a, b), &[start, end])) in copy.wires.iter().zip(&source.wires).enumerate() {
        if !visited.contains(&index) {
            visited.extend(wire_path(copy, index));
            selection.insert(Item::Wire(circuit.wires.len()));
        }
        circuit.wires.push(Wire(a.offset(delta), b.offset(delta)));
        links.wires.push([id(start), id(end)]);
    }
    links.bends.extend(source.bends.iter().copied().map(id));
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cirtam_core::{circuit::Kind, persistence, topology::MAX_NODE_ID};

    fn imported_max_ground() -> Circuit {
        let fixture = format!(
            r#"{{
            "format": "cirtam", "version": 2,
            "circuit": {{
                "components": [{{"kind":"Ground","at":[0,0],"rotation":0,"label":"","value":0}}],
                "wires": [],
                "connections": {{"terminals":[[{MAX_NODE_ID}]],"wires":[]}}
            }}
        }}"#
        );
        persistence::decode(&fixture).unwrap()
    }

    fn assert_round_trip(circuit: &Circuit) {
        let saved = persistence::encode(circuit).unwrap();
        assert_eq!(&persistence::decode(&saved).unwrap(), circuit);
    }

    #[test]
    fn placing_two_terminals_after_importing_the_maximum_stays_savable() {
        let mut circuit = imported_max_ground();
        add_component(&mut circuit, Component::new(Kind::Resistor, Point(6, 0))).unwrap();
        let links = circuit.connections.as_ref().unwrap();
        assert_eq!(links.terminals, [vec![MAX_NODE_ID], vec![0, 1]]);
        assert_round_trip(&circuit);
    }

    #[test]
    fn direct_join_at_the_ceiling_reuses_existing_ids() {
        let mut circuit = imported_max_ground();
        add_component(&mut circuit, Component::new(Kind::Resistor, Point(0, 0))).unwrap();
        let pin = circuit.components[0].terminal(0);
        let links = circuit.connections.as_ref().unwrap();
        let [joined, other] = [links.terminals[1][0], links.terminals[1][1]];
        connect(
            &mut circuit,
            Target::Node(MAX_NODE_ID, pin),
            &[],
            Target::Node(joined, pin),
        )
        .unwrap();
        assert_eq!(
            circuit.connections.as_ref().unwrap().terminals,
            [vec![MAX_NODE_ID], vec![MAX_NODE_ID, other]]
        );
        assert!(circuit.wires.is_empty());
        assert_round_trip(&circuit);
    }

    #[test]
    fn paste_at_the_ceiling_preserves_terminal_wire_and_bend_mapping() {
        let mut circuit = imported_max_ground();
        add_component(&mut circuit, Component::new(Kind::Resistor, Point(6, 0))).unwrap();
        let from = circuit.components[0].terminal(0);
        let to = circuit.components[1].terminal(0);
        let resistor_id = circuit.connections.as_ref().unwrap().terminals[1][0];
        connect(
            &mut circuit,
            Target::Node(MAX_NODE_ID, from),
            &[Point(1, 2)],
            Target::Node(resistor_id, to),
        )
        .unwrap();

        let selection = HashSet::from([Item::Component(0), Item::Component(1), Item::Wire(0)]);
        let fragment = copy(&circuit, &selection);
        let original = circuit.connections.as_ref().unwrap().clone();
        paste(&mut circuit, &fragment).unwrap();
        let links = circuit.connections.as_ref().unwrap();
        assert_eq!(links.terminals[..2], original.terminals);
        assert_eq!(links.wires[..2], original.wires);
        let [start, bend] = links.wires[2];
        let [next, end] = links.wires[3];
        assert_eq!(bend, next);
        assert!(links.bends.contains(&bend));
        assert_eq!(links.terminals[2][0], start);
        assert_eq!(links.terminals[3][0], end);
        let occupied: HashSet<_> = original
            .terminals
            .iter()
            .flatten()
            .chain(original.wires.iter().flatten())
            .collect();
        assert!(links.terminals[2..]
            .iter()
            .flatten()
            .chain(links.wires[2..].iter().flatten())
            .all(|id| *id <= MAX_NODE_ID && !occupied.contains(id)));
        assert_round_trip(&circuit);
    }

    #[test]
    fn bridging_free_ends_preserves_geometry_and_saves_one_selectable_path() {
        let left = Wire(Point(0, 0), Point(2, 0));
        let right = Wire(Point(4, 0), Point(6, 0));
        let mut circuit = Circuit {
            wires: vec![left, right],
            ..Default::default()
        };
        circuit.make_connections_explicit();
        let links = circuit.connections.as_ref().unwrap();
        let start = links.wires[0][1];
        let end = links.wires[1][0];

        connect(
            &mut circuit,
            Target::Node(start, left.1),
            &[],
            Target::Node(end, right.0),
        )
        .unwrap();

        let saved = cirtam_core::persistence::encode(&circuit).unwrap();
        let reopened = cirtam_core::persistence::decode(&saved).unwrap();
        assert_eq!(reopened, circuit);
        assert_eq!(reopened.wires, [left, right, Wire(left.1, right.0)]);
        let bends = &reopened.connections.as_ref().unwrap().bends;
        assert!(bends.contains(&start) && bends.contains(&end));
        for segment in 0..3 {
            assert_eq!(wire_path(&reopened, segment), HashSet::from([0, 1, 2]));
        }
    }

    #[test]
    fn bridging_keeps_branch_and_terminal_joins_visible() {
        for with_terminal in [false, true] {
            let left = Wire(Point(0, 0), Point(2, 0));
            let right = Wire(Point(4, 0), Point(6, 0));
            let mut circuit = Circuit {
                wires: vec![left, right],
                ..Default::default()
            };
            if with_terminal {
                circuit.components.push(Component::new(
                    cirtam_core::circuit::Kind::Ground,
                    Point(6, 0),
                ));
            } else {
                circuit.wires.push(Wire(right.0, Point(4, 2)));
            }
            circuit.make_connections_explicit();
            let links = circuit.connections.as_ref().unwrap();
            let start = links.wires[0][1];
            let end = links.wires[1][0];

            connect(
                &mut circuit,
                Target::Node(start, left.1),
                &[],
                Target::Node(end, right.0),
            )
            .unwrap();

            let bends = &circuit.connections.as_ref().unwrap().bends;
            assert!(bends.contains(&start));
            assert!(!bends.contains(&end));
            assert_eq!(counts(&circuit)[&end].1, 3);
        }
    }

    fn junction() -> (Circuit, Node) {
        let mut circuit = Circuit {
            components: vec![Component::new(Kind::Ground, Point(-10, 0))],
            wires: vec![
                Wire(Point(0, 0), Point(6, 0)),
                Wire(Point(2, 3), Point(4, 3)),
            ],
            ..Default::default()
        };
        circuit.make_connections_explicit();
        circuit.connections.as_mut().unwrap().terminals[0][0] = MAX_NODE_ID;
        let start = circuit.connections.as_ref().unwrap().wires[1][0];
        connect(
            &mut circuit,
            Target::Node(start, Point(2, 3)),
            &[Point(3, 2)],
            Target::Interior(0, Point(3, 0)),
        )
        .unwrap();
        let links = circuit.connections.as_ref().unwrap();
        let junction = links.wires[0][1];
        assert_eq!(links.wires[2][0], junction);
        assert_eq!(links.wires[4][1], junction);
        let bend = links.wires[3][1];
        assert_eq!(links.wires[4][0], bend);
        assert_ne!(bend, junction);
        assert!(links.bends.contains(&bend));
        assert_round_trip(&circuit);
        (circuit, junction)
    }

    #[test]
    fn splits_moves_and_simplifies_a_middle_junction() {
        let (circuit, junction) = junction();
        assert_eq!(counts(&circuit)[&junction], (Point(3, 0), 3));
        let mut moved = translated(
            &circuit,
            &HashSet::from([Item::Node(junction)]),
            Point(0, 1),
        )
        .unwrap();
        assert_eq!(counts(&moved)[&junction], (Point(3, 1), 3));
        moved.validate_geometry().unwrap();
        delete(&mut moved, &HashSet::from([Item::Wire(4)])).unwrap();
        assert_eq!(counts(&moved)[&junction], (Point(3, 1), 2));
        assert!(moved
            .connections
            .as_ref()
            .unwrap()
            .bends
            .contains(&junction));
        assert_eq!(moved.wires[0], Wire(Point(0, 0), Point(3, 1)));
        assert_eq!(moved.wires[2], Wire(Point(3, 1), Point(6, 0)));
        assert_eq!(wire_path(&moved, 0), HashSet::from([0, 2]));
        let saved = cirtam_core::persistence::encode(&moved).unwrap();
        assert_eq!(cirtam_core::persistence::decode(&saved).unwrap(), moved);
        moved.validate_geometry().unwrap();
    }

    #[test]
    fn deliberate_disconnect_survives_round_trip_and_paste() {
        let (mut circuit, junction) = junction();
        disconnect(&mut circuit, &HashSet::from([junction])).unwrap();
        let links = circuit.connections.as_ref().unwrap();
        let original_nodes: HashSet<_> = links.endpoints(&circuit).map(|(node, _)| node).collect();
        let ids: HashSet<_> = links
            .endpoints(&circuit)
            .filter_map(|(node, point)| (point == Point(3, 0)).then_some(node))
            .collect();
        assert_eq!(ids.len(), 3);
        let text = cirtam_core::persistence::encode(&circuit).unwrap();
        assert_eq!(cirtam_core::persistence::decode(&text).unwrap(), circuit);

        let items = (0..circuit.wires.len()).map(Item::Wire).collect();
        let fragment = copy(&circuit, &items);
        let before = circuit.wires.len();
        let pasted = paste(&mut circuit, &fragment).unwrap();
        assert_eq!(pasted.len(), before - 1);
        assert_eq!(selected_segments(&circuit, &pasted).len(), before);
        let links = circuit.connections.as_ref().unwrap();
        let pasted_ids = &links.wires[before..];
        assert!(pasted_ids
            .iter()
            .flatten()
            .all(|node| !original_nodes.contains(node)));
        assert_eq!(pasted_ids[3][1], pasted_ids[4][0]);
        assert_round_trip(&circuit);
    }

    #[test]
    fn moving_component_off_a_shared_junction_keeps_branches_stationary() {
        let resistor = Component::new(cirtam_core::circuit::Kind::Resistor, Point(0, 0));
        let pin = resistor.terminal(1);
        let mut circuit = Circuit {
            components: vec![
                resistor.clone(),
                resistor,
                Component::new(Kind::Ground, Point(-10, 0)),
            ],
            wires: vec![Wire(pin, Point(8, 0)), Wire(pin, Point(2, 5))],
            ..Default::default()
        };
        circuit.make_connections_explicit();
        circuit.connections.as_mut().unwrap().terminals[2][0] = MAX_NODE_ID;
        let junction = circuit.connections.as_ref().unwrap().terminals[0][1];
        let selection = HashSet::from([Item::Component(0), Item::Component(1)]);
        let moved = translated(&circuit, &selection, Point(1, 2)).unwrap();
        assert_eq!(&moved.wires[..2], circuit.wires.as_slice());
        assert_eq!(moved.wires[2], Wire(pin, Point(3, 2)));
        let links = moved.connections.as_ref().unwrap();
        assert_eq!(links.wires[2], [junction, links.terminals[0][1]]);
        assert_eq!(links.terminals[0][1], links.terminals[1][1]);
        assert_eq!(links.terminals[2][0], MAX_NODE_ID);
        assert_eq!(moved.wires.len(), 3);
        assert_round_trip(&moved);
    }

    #[test]
    fn attached_wire_endpoint_follows_component() {
        let mut circuit = Circuit {
            components: vec![Component::new(
                cirtam_core::circuit::Kind::Resistor,
                Point(0, 0),
            )],
            wires: vec![Wire(Point(2, 0), Point(5, 0))],
            ..Default::default()
        };
        circuit.make_connections_explicit();
        let moved =
            translated(&circuit, &HashSet::from([Item::Component(0)]), Point(1, 2)).unwrap();
        assert_eq!(moved.wires[0], Wire(Point(3, 2), Point(5, 0)));
        assert_eq!(
            moved.connections.as_ref().unwrap().terminals[0][1],
            moved.connections.as_ref().unwrap().wires[0][0]
        );
        moved.validate_geometry().unwrap();
    }
}
