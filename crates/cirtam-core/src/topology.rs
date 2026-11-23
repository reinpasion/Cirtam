use crate::circuit::{Circuit, Point, Wire};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

pub type Node = u64;
pub const MAX_NODE_ID: Node = (1 << 63) - 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connections {
    pub terminals: Vec<Vec<Node>>,
    pub wires: Vec<[Node; 2]>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub bends: BTreeSet<Node>,
}

impl Connections {
    pub fn node_allocator(&self) -> NodeAllocator {
        let occupied: HashSet<_> = self
            .terminals
            .iter()
            .flatten()
            .chain(self.wires.iter().flatten())
            .chain(&self.bends)
            .copied()
            .collect();
        let next = occupied
            .iter()
            .copied()
            .max()
            .filter(|&node| node < MAX_NODE_ID)
            .map_or(0, |node| node + 1);
        NodeAllocator { occupied, next }
    }

    pub fn endpoints<'a>(
        &'a self,
        circuit: &'a Circuit,
    ) -> impl Iterator<Item = (Node, Point)> + 'a {
        self.terminals
            .iter()
            .zip(&circuit.components)
            .flat_map(|(ids, component)| ids.iter().copied().zip(component.terminals()))
            .chain(
                self.wires
                    .iter()
                    .zip(&circuit.wires)
                    .flat_map(|(&[a, b], &Wire(p, q))| [(a, p), (b, q)]),
            )
    }

    pub(crate) fn validate(&self, circuit: &Circuit) -> Result<(), String> {
        if self.terminals.len() != circuit.components.len()
            || self.wires.len() != circuit.wires.len()
            || self
                .terminals
                .iter()
                .zip(&circuit.components)
                .any(|(ids, component)| ids.len() != component.terminals().count())
        {
            return Err("Connection records do not match the circuit elements.".into());
        }
        let mut positions = HashMap::new();
        for (node, point) in self.endpoints(circuit) {
            if node > MAX_NODE_ID
                || positions
                    .insert(node, point)
                    .is_some_and(|previous| previous != point)
            {
                return Err(
                    "A connection node has an invalid identity or conflicting positions.".into(),
                );
            }
        }
        for &bend in &self.bends {
            if self.terminals.iter().flatten().any(|&node| node == bend)
                || self
                    .wires
                    .iter()
                    .flatten()
                    .filter(|&&node| node == bend)
                    .count()
                    != 2
            {
                return Err("A bend must join two wire ends without a component terminal.".into());
            }
        }
        Ok(())
    }
}

/// Reserves existing and newly issued IDs for one edit.
pub struct NodeAllocator {
    occupied: HashSet<Node>,
    next: Node,
}

impl NodeAllocator {
    pub fn allocate(&mut self) -> Result<Node, String> {
        let start = self.next;
        loop {
            let candidate = self.next;
            self.next = if candidate == MAX_NODE_ID {
                0
            } else {
                candidate + 1
            };
            if self.occupied.insert(candidate) {
                return Ok(candidate);
            }
            if self.next == start {
                return Err("No unused connection node IDs remain for this edit.".into());
            }
        }
    }
}

impl Circuit {
    /// Import old coordinate-based connections once, including legacy T-junctions.
    pub fn make_connections_explicit(&mut self) {
        if self.connections.is_some() {
            return;
        }
        let mut positions = HashMap::new();
        let mut id = |point| {
            let next = positions.len() as Node;
            *positions.entry(point).or_insert(next)
        };
        let terminals = self
            .components
            .iter()
            .map(|part| part.terminals().map(&mut id).collect())
            .collect();
        for &Wire(a, b) in &self.wires {
            id(a);
            id(b);
        }

        let mut wires = Vec::new();
        let mut connections = Connections {
            terminals,
            ..Default::default()
        };
        for &wire in &self.wires {
            let mut points: Vec<_> = positions
                .keys()
                .copied()
                .filter(|&p| wire.contains(p))
                .collect();
            points.sort_unstable_by_key(|p| {
                let x = i64::from(p.0) - i64::from(wire.0 .0);
                let y = i64::from(p.1) - i64::from(wire.0 .1);
                x * x + y * y
            });
            for segment in points.windows(2) {
                let [a, b] = [segment[0], segment[1]];
                wires.push(Wire(a, b));
                connections.wires.push([positions[&a], positions[&b]]);
            }
        }
        self.wires = wires;
        self.connections = Some(connections);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence;

    #[test]
    fn allocator_crosses_the_ceiling_without_reissuing_occupied_or_reserved_ids() {
        let connections = Connections {
            terminals: vec![vec![MAX_NODE_ID - 1, 0], vec![2, 4]],
            ..Default::default()
        };
        let mut allocator = connections.node_allocator();
        let issued = (0..5)
            .map(|_| allocator.allocate().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(issued, [MAX_NODE_ID, 1, 3, 5, 6]);
        assert!(issued.iter().all(|&node| node <= MAX_NODE_ID));
    }

    #[test]
    fn imported_node_above_the_maximum_is_rejected() {
        let document = serde_json::json!({
            "format": "cirtam",
            "version": 2,
            "circuit": {
                "components": [crate::circuit::Component::new(crate::circuit::Kind::Ground, Point(0, 0))],
                "wires": [],
                "connections": { "terminals": [[MAX_NODE_ID + 1]], "wires": [] }
            }
        });
        assert!(persistence::decode(&document.to_string())
            .unwrap_err()
            .contains("invalid identity"));
    }
}
