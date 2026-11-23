use crate::app::{Editor, Item, Tool};
use cirtam_core::circuit::{Component, Kind, Point, Wire};
use eframe::egui::{
    vec2, CentralPanel, Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect,
};

fn frame(ctx: &Context, editor: &mut Editor, events: Vec<Event>) -> Pos2 {
    let mut center = Pos2::ZERO;
    let _ = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        },
        |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                center = ui.available_rect_before_wrap().center();
                crate::canvas::show(editor, ui);
            });
        },
    );
    center
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn drag(ctx: &Context, editor: &mut Editor, start: Pos2, end: Pos2) {
    frame(
        ctx,
        editor,
        vec![Event::PointerMoved(start), button(start, true)],
    );
    frame(ctx, editor, vec![Event::PointerMoved(end)]);
    frame(ctx, editor, vec![button(end, false)]);
}

fn click(ctx: &Context, editor: &mut Editor, pos: Pos2) {
    frame(
        ctx,
        editor,
        vec![Event::PointerMoved(pos), button(pos, true)],
    );
    frame(ctx, editor, vec![button(pos, false)]);
}

fn desktop_frame(ctx: &Context, editor: &mut Editor, events: Vec<Event>) -> Pos2 {
    let mut center = Pos2::ZERO;
    let _ = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        },
        |ctx| {
            editor.frame(ctx);
            center = ctx
                .read_response(editor.view.focus_id.unwrap())
                .unwrap()
                .rect
                .center();
        },
    );
    center
}

fn desktop_click(ctx: &Context, editor: &mut Editor, pos: Pos2) {
    desktop_frame(
        ctx,
        editor,
        vec![Event::PointerMoved(pos), button(pos, true)],
    );
    desktop_frame(ctx, editor, vec![button(pos, false)]);
}

fn key(key: eframe::egui::Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn resistor_pair() -> Editor {
    let mut editor = Editor::default();
    for x in [-5, 5] {
        crate::wiring::add_component(
            &mut editor.circuit,
            Component::new(Kind::Resistor, Point(x, 0)),
        )
        .unwrap();
    }
    editor
}

#[test]
fn rapid_body_clicks_select_and_delete_the_last_component() {
    let ctx = Context::default();
    let mut editor = resistor_pair();
    let center = desktop_frame(&ctx, &mut editor, vec![]);
    for x in [-110.0, 110.0, 110.0] {
        desktop_click(&ctx, &mut editor, center + vec2(x, 0.0));
        assert_eq!(editor.selection.len(), 1);
        assert!(editor
            .selection
            .contains(&Item::Component(usize::from(x > 0.0))));
        assert_eq!(ctx.memory(|memory| memory.focused()), editor.view.focus_id);
    }

    desktop_frame(
        &ctx,
        &mut editor,
        vec![key(eframe::egui::Key::Delete, Modifiers::NONE)],
    );
    assert_eq!(editor.circuit.components.len(), 1);
    assert_eq!(editor.circuit.components[0].at, Point(-5, 0));
}

#[test]
fn only_two_clicks_on_the_same_value_focus_its_editor() {
    use eframe::egui::Key;

    let ctx = Context::default();
    let mut editor = resistor_pair();
    let center = desktop_frame(&ctx, &mut editor, vec![]);
    let left = center + vec2(-110.0, 24.0);
    let right = center + vec2(110.0, 24.0);
    desktop_click(&ctx, &mut editor, left);
    desktop_click(&ctx, &mut editor, right);
    assert_eq!(editor.selection.len(), 1);
    assert!(editor.selection.contains(&Item::Component(1)));
    assert_eq!(ctx.memory(|memory| memory.focused()), editor.view.focus_id);

    desktop_click(&ctx, &mut editor, right);
    let focused = ctx.memory(|memory| memory.focused());
    assert!(focused.is_some() && focused != editor.view.focus_id);
    desktop_frame(
        &ctx,
        &mut editor,
        vec![key(Key::A, Modifiers::COMMAND), Event::Text("2200".into())],
    );
    desktop_frame(&ctx, &mut editor, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(editor.circuit.components[0].value, 1000.0);
    assert_eq!(editor.circuit.components[1].value, 2200.0);
}

#[test]
fn wire_tool_snaps_to_component_terminals() {
    let ctx = Context::default();
    let mut editor = Editor::default();
    editor.circuit.components = vec![
        Component::new(Kind::Resistor, Point(0, 0)),
        Component::new(Kind::Ground, Point(7, 2)),
    ];
    editor.circuit.connections = None;
    editor.circuit.make_connections_explicit();
    let center = frame(&ctx, &mut editor, vec![]);
    let start = center + vec2(46.0, 1.0);
    click(&ctx, &mut editor, start);
    assert!(matches!(
        editor.tool,
        Tool::Wire(Some(crate::wiring::Target::Node(_, Point(2, 0))))
    ));
    click(&ctx, &mut editor, start);
    click(&ctx, &mut editor, center + vec2(88.0, 66.0));
    assert!(matches!(
        editor.tool,
        Tool::Wire(Some(crate::wiring::Target::Node(_, Point(2, 0))))
    ));
    assert!(editor.circuit.wires.is_empty());
    assert_eq!(editor.route_corners, [Point(4, 3)]);
    click(&ctx, &mut editor, center + vec2(110.0, 46.0));
    assert!(matches!(editor.tool, Tool::Select));
    assert_eq!(
        editor.circuit.wires,
        [
            Wire(Point(2, 0), Point(4, 3)),
            Wire(Point(4, 3), Point(5, 2))
        ]
    );
}

#[test]
fn box_direction_controls_partial_selection() {
    let ctx = Context::default();
    let mut editor = Editor::default();
    editor.circuit.components = vec![Component::new(Kind::Resistor, Point(0, 0))];
    editor.circuit.wires = vec![Wire(Point(3, 1), Point(3, 5))];
    editor.circuit.connections = None;
    editor.circuit.make_connections_explicit();
    let center = frame(&ctx, &mut editor, vec![]);
    drag(
        &ctx,
        &mut editor,
        center + vec2(8.0, -80.0),
        center + vec2(90.0, 60.0),
    );
    assert!(editor.selection.is_empty());
    drag(
        &ctx,
        &mut editor,
        center + vec2(90.0, -80.0),
        center + vec2(8.0, 60.0),
    );
    assert_eq!(editor.selection.len(), 2);
    assert!(editor.selection.contains(&Item::Component(0)));
    assert!(editor.selection.contains(&Item::Wire(0)));
}

#[test]
fn placed_component_keeps_preview_orientation() {
    let ctx = Context::default();
    let mut editor = Editor::default();
    editor.tool = Tool::Place {
        kind: Kind::Voltage,
        rotation: 3,
    };
    let center = frame(&ctx, &mut editor, vec![]);
    click(&ctx, &mut editor, center);
    assert_eq!(editor.circuit.components.len(), 1);
    let placed = &editor.circuit.components[0];
    assert_eq!(placed.rotation, 3);
    assert_eq!(placed.terminal(0), Point(0, 2));
}

#[test]
fn scroll_wheel_changes_reported_zoom() {
    let ctx = Context::default();
    let mut editor = Editor::default();
    let center = frame(&ctx, &mut editor, vec![]);
    assert_eq!(editor.view.zoom_percent(), 100.0);
    frame(
        &ctx,
        &mut editor,
        vec![
            Event::PointerMoved(center),
            Event::MouseWheel {
                unit: eframe::egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 120.0),
                modifiers: Modifiers::NONE,
            },
        ],
    );
    assert!(editor.view.zoom_percent() > 100.0);
}

fn disconnected_overlap(extra_terminal: bool) -> Editor {
    let mut editor = Editor::default();
    crate::wiring::add_component(
        &mut editor.circuit,
        Component::new(Kind::Resistor, Point(2, 0)),
    )
    .unwrap();
    if extra_terminal {
        crate::wiring::add_component(
            &mut editor.circuit,
            Component::new(Kind::Ground, Point(2, 0)),
        )
        .unwrap();
    }
    editor.circuit.wires.push(Wire(Point(0, 0), Point(-4, 0)));
    let links = editor.circuit.connections.as_mut().unwrap();
    let mut allocator = links.node_allocator();
    links
        .wires
        .push([allocator.allocate().unwrap(), allocator.allocate().unwrap()]);
    editor
}

#[test]
fn double_click_reconnects_two_overlapping_nodes() {
    let ctx = Context::default();
    let mut editor = disconnected_overlap(false);
    let center = frame(&ctx, &mut editor, vec![]);
    let before = editor.circuit.connections.as_ref().unwrap().clone();
    assert_ne!(before.terminals[0][0], before.wires[0][0]);
    click(&ctx, &mut editor, center);
    assert_eq!(editor.circuit.connections.as_ref().unwrap(), &before);
    assert!(matches!(editor.tool, Tool::Select));
    click(&ctx, &mut editor, center);
    let after = editor.circuit.connections.as_ref().unwrap();
    assert_eq!(after.terminals[0][0], after.wires[0][0]);
    assert_eq!(after.terminals[0][1], before.terminals[0][1]);
    assert_eq!(after.wires[0][1], before.wires[0][1]);
    assert!(matches!(editor.tool, Tool::Select));
}

#[test]
fn double_click_does_not_join_three_overlapping_nodes() {
    let ctx = Context::default();
    let mut editor = disconnected_overlap(true);
    let center = frame(&ctx, &mut editor, vec![]);
    let before = editor.circuit.clone();
    click(&ctx, &mut editor, center);
    click(&ctx, &mut editor, center);
    assert_eq!(editor.circuit, before);
    assert!(editor.notice.contains("More than two"));
}
