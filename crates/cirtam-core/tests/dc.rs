use cirtam_core::{
    circuit::{Circuit, Component, Kind, Point, Wire},
    diagnostics::{Location, Severity},
    persistence, solver,
};

fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
}

#[test]
fn voltage_divider_with_ideal_meters_round_trips() {
    let circuit = persistence::decode(include_str!("../../../examples/divider.crtm")).unwrap();
    let saved = persistence::encode(&circuit).unwrap();
    assert_eq!(circuit, persistence::decode(&saved).unwrap());
    let readings = solver::solve(&circuit).unwrap().readings;
    near(readings[0].current.unwrap(), -0.005);
    near(readings[3].voltage, 0.0);
    near(readings[3].current.unwrap(), 0.005);
    near(readings[4].voltage, 5.0);
    near(readings[4].current.unwrap(), 0.0);
    assert_eq!(readings[5].current, None);
}

#[test]
fn malformed_fixed_component_fields_load_but_are_local_solve_and_save_errors() {
    let divider = include_str!("../../../examples/divider.crtm");
    let valid = persistence::decode(divider).unwrap();
    solver::solve(&valid).unwrap();
    persistence::encode(&valid).unwrap();

    for (kind, field, value, expected_error) in [
        (
            "Ground",
            "actual_ohms",
            1,
            "A sampled resistance requires resistor tolerance",
        ),
        ("Ground", "value", 1, "stored value must be zero"),
        ("Ammeter", "value", -1, "stored value must be zero"),
        ("Voltmeter", "value", 1, "stored value must be zero"),
    ] {
        let mut document: serde_json::Value = serde_json::from_str(divider).unwrap();
        let components = document["circuit"]["components"].as_array_mut().unwrap();
        let index = components
            .iter()
            .position(|component| component["kind"] == kind)
            .unwrap();
        components[index][field] = value.into();

        let malformed = persistence::decode(&document.to_string()).unwrap();
        let component = &malformed.components[index];
        if field == "actual_ohms" {
            assert_eq!(component.actual_ohms, Some(f64::from(value)));
        } else {
            assert_eq!(component.value, f64::from(value));
        }
        let diagnostics = solver::solve(&malformed).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Serious
                    && diagnostic.location == Some(Location::Component(index))
                    && diagnostic.message.contains(expected_error)),
            "{kind} with malformed {field} should have a local diagnostic"
        );
        assert!(persistence::encode(&malformed)
            .unwrap_err()
            .contains(expected_error));
    }
}

#[test]
fn signed_sources_solve_and_round_trip() {
    for (example, expected_current) in [
        (include_str!("../../../examples/divider.crtm"), -0.005),
        (include_str!("../../../examples/current-load.crtm"), -0.002),
    ] {
        let mut circuit = persistence::decode(example).unwrap();
        assert!(matches!(
            circuit.components[0].kind,
            Kind::Voltage | Kind::Current
        ));
        circuit.components[0].value *= -1.0;
        let encoded = persistence::encode(&circuit).unwrap();
        let reopened = persistence::decode(&encoded).unwrap();
        assert_eq!(reopened, circuit);
        near(
            solver::solve(&reopened).unwrap().readings[1]
                .current
                .unwrap(),
            expected_current,
        );
    }
}

#[test]
fn current_source_drives_resistive_load() {
    let circuit = persistence::decode(include_str!("../../../examples/current-load.crtm")).unwrap();
    let readings = solver::solve(&circuit).unwrap().readings;
    near(readings[0].voltage, -2.0);
    near(readings[1].current.unwrap(), 0.002);
}

#[test]
fn diagnoses_missing_ground_floating_and_short() {
    let mut circuit = Circuit {
        connections: None,
        components: vec![Component::new(Kind::Voltage, Point(0, 0))],
        wires: vec![],
    };
    assert!(solver::solve(&circuit)
        .unwrap_err()
        .iter()
        .any(|d| d.location.is_none() && d.message.contains("Missing ground")));
    circuit
        .components
        .push(Component::new(Kind::Ground, Point(0, 4)));
    assert!(solver::solve(&circuit)
        .unwrap_err()
        .iter()
        .any(|d| d.location == Some(Location::Component(0)) && d.message.contains("floating")));
    circuit.wires.push(Wire(Point(-2, 0), Point(2, 0)));
    assert!(solver::solve(&circuit)
        .unwrap_err()
        .iter()
        .any(|d| d.location == Some(Location::Component(0)) && d.message.contains("shorted")));
}

#[test]
fn validates_si_prefixes_and_versions() {
    for (input, expected) in [("1k", 1000.0), ("2M", 2e6), ("3m", 0.003), ("4u", 4e-6)] {
        near(Kind::Resistor.parse_value(input).unwrap(), expected);
    }
    for input in ["0", "-10", "3kk", "Inf", "oops", "1pp", "1nn"] {
        assert!(Kind::Resistor.parse_value(input).is_err());
    }
    assert!(persistence::decode(r#"{"format":"cirtam","version":2,"circuit":{}}"#).is_err());
}

#[test]
fn old_identifier_opens_with_unspecified_ratings_and_overloads_are_local() {
    let renamed = include_str!("../../../examples/divider.crtm");
    let legacy = renamed.replace("\"cirtam\"", "\"cirmat\"");
    let mut circuit = persistence::decode(&legacy).unwrap();
    assert!(circuit
        .components
        .iter()
        .all(|c| c.ratings.power_watts.is_none() && c.ratings.tolerance_percent.is_none()));
    let nominal = solver::solve(&circuit).unwrap();
    assert!(nominal.diagnostics.is_empty());
    near(nominal.readings[1].resistor_power.unwrap(), 0.025);
    assert!(nominal.readings[3].resistor_power.is_none());

    circuit.components[1].ratings.power_watts = Some(0.01);
    circuit.components[1].ratings.tolerance_percent = Some(5.0);
    let result = solver::solve(&circuit).unwrap();
    assert_eq!(nominal.readings[1].voltage, result.readings[1].voltage);
    assert_eq!(nominal.readings[1].current, result.readings[1].current);
    assert_eq!(
        nominal.readings[1].resistor_power,
        result.readings[1].resistor_power
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Serious
            && d.location == Some(Location::Component(1))
            && d.message.contains("exceeds")));
    let saved = persistence::encode(&circuit).unwrap();
    assert!(saved.contains("\"cirtam\""));
    assert_eq!(persistence::decode(&saved).unwrap(), circuit);
}

#[test]
fn invalid_rating_and_disconnected_wire_both_have_locations() {
    let mut circuit = persistence::decode(include_str!("../../../examples/divider.crtm")).unwrap();
    circuit.components[1].ratings.tolerance_percent = Some(-1.0);
    let wire_index = circuit.wires.len();
    circuit.wires.push(Wire(Point(100, 100), Point(103, 100)));
    let diagnostics = solver::solve(&circuit).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|d| d.location == Some(Location::Component(1)) && d.message.contains("Tolerance")));
    assert!(diagnostics
        .iter()
        .any(|d| d.location == Some(Location::Wire(wire_index))));
    assert!(persistence::encode(&circuit).is_err());
}

#[test]
fn legacy_migration_preserves_readings_and_crossings_do_not_join_explicit_nets() {
    let mut circuit = persistence::decode(include_str!("../../../examples/divider.crtm")).unwrap();
    let before = solver::solve(&circuit).unwrap();
    circuit.make_connections_explicit();
    let migrated = solver::solve(&circuit).unwrap();
    for (left, right) in before.readings.iter().zip(migrated.readings) {
        near(left.voltage, right.voltage);
        assert_eq!(left.current, right.current);
    }
    let text = persistence::encode(&circuit).unwrap();
    assert!(text.contains("\"version\": 2"));
    assert_eq!(persistence::decode(&text).unwrap(), circuit);

    let index = circuit.wires.len();
    let links = circuit.connections.as_mut().unwrap();
    let mut allocator = links.node_allocator();
    let start = allocator.allocate().unwrap();
    let end = allocator.allocate().unwrap();
    // The endpoint lies in the middle of an existing segment, and the stroke crosses another.
    circuit.wires.push(Wire(Point(-4, -5), Point(-8, -3)));
    links.wires.push([start, end]);
    let diagnostics = solver::solve(&circuit).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|issue| issue.location == Some(Location::Wire(index))
            && issue.message.contains("disconnected")));
}

#[test]
fn mixed_version_v1_preserves_explicit_connections_and_saves_as_v2() {
    let source = 0;
    let load = 1;
    let mut circuit =
        persistence::decode(include_str!("../../../examples/current-load.crtm")).unwrap();
    assert_eq!(circuit.components[source].kind, Kind::Current);
    assert_eq!(circuit.components[load].value, 1000.0);
    circuit.make_connections_explicit();
    near(
        solver::solve(&circuit).unwrap().readings[load]
            .current
            .unwrap(),
        0.002,
    );

    let point = circuit.components[source].terminal(0);
    let links = circuit.connections.as_mut().unwrap();
    assert_eq!(point, circuit.wires[1].0);
    assert_eq!(links.terminals[source][0], links.wires[1][0]);
    links.terminals[source][0] = links.node_allocator().allocate().unwrap();
    assert_ne!(links.terminals[source][0], links.wires[1][0]);

    let v2_text = persistence::encode(&circuit).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&v2_text).unwrap();
    assert_eq!(v2["version"], 2);
    let control = persistence::decode(&v2_text).unwrap();
    assert_eq!(control, circuit);
    let floating_terminal = |circuit: &Circuit| {
        solver::solve(circuit)
            .unwrap_err()
            .into_iter()
            .find(|diagnostic| {
                diagnostic.severity == Severity::Serious
                    && diagnostic.location == Some(Location::Component(source))
                    && diagnostic.message.starts_with("Terminal 1 is floating.")
            })
            .expect("source terminal 1 should be floating")
    };
    let expected = floating_terminal(&control);

    let mut v1 = v2.clone();
    v1["version"] = 1.into();
    for format in ["cirtam", "cirmat"] {
        v1["format"] = format.into();
        let mut opened = persistence::decode(&v1.to_string()).unwrap();
        assert_eq!(
            opened, control,
            "opening {format} v1 must preserve node IDs"
        );
        opened.make_connections_explicit();
        assert_eq!(
            opened, control,
            "materializing {format} v1 must preserve node IDs"
        );
        assert_eq!(floating_terminal(&opened).message, expected.message);

        let saved = persistence::encode(&opened).unwrap();
        let saved_header: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(saved_header["format"], "cirtam");
        assert_eq!(saved_header["version"], 2);
        let reopened = persistence::decode(&saved).unwrap();
        assert_eq!(reopened, control);
        assert_eq!(floating_terminal(&reopened).message, expected.message);
    }

    v1["format"] = "cirtam".into();
    assert!(v1["circuit"]
        .as_object_mut()
        .unwrap()
        .remove("connections")
        .is_some());
    let coordinate_only = persistence::decode(&v1.to_string()).unwrap();
    assert_eq!(coordinate_only.components, control.components);
    assert_eq!(coordinate_only.wires, control.wires);
    near(
        solver::solve(&coordinate_only).unwrap().readings[load]
            .current
            .unwrap(),
        0.002,
    );

    v1["version"] = 2.into();
    assert_eq!(
        persistence::decode(&v1.to_string()).unwrap_err(),
        "Version 2 requires explicit connection records."
    );
}

#[test]
fn stored_resistance_drives_dc_and_round_trips_without_resampling() {
    let mut circuit = persistence::decode(include_str!("../../../examples/divider.crtm")).unwrap();
    let resistor = &mut circuit.components[1];
    resistor.ratings.tolerance_percent = Some(10.0);
    let actual: f64 = 1050.1234567890123;
    resistor.actual_ohms = Some(actual);
    let nominal_other = circuit.components[2].value;
    let encoded = persistence::encode(&circuit).unwrap();
    let reopened = persistence::decode(&encoded).unwrap();
    assert_eq!(
        reopened.components[1].actual_ohms.unwrap().to_bits(),
        actual.to_bits()
    );

    let current = 10.0 / (actual + nominal_other);
    for circuit in [&circuit, &reopened, &reopened] {
        let results = solver::solve(circuit).unwrap();
        near(results.readings[1].current.unwrap(), current);
        near(results.readings[1].voltage, current * actual);
        near(
            results.readings[1].resistor_power.unwrap(),
            current * current * actual,
        );
    }
}
