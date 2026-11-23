# Cirtam

Cirtam is a circuit simulator built to expand gradually from fundamental circuits toward topics in electrical, electronics, and computer engineering. It currently provides a graphical editor and linear DC operating-point solver for basic circuits. Cirtam aims to run well on modest hardware, with broader professional use cases as a long-term goal.

## Limitations

- Supports linear DC operating-point analysis only.
- Supports resistors, independent DC voltage and current sources, ground, and ideal voltmeters and ammeters.
- Solving is limited to 512 components and 2,048 wire segments.
- Capacitors, inductors, LEDs, microcontrollers, and other advanced components are not supported yet.
- The desktop app currently runs the solver synchronously.

## Set-up

Install a stable Rust toolchain, then open a terminal in the project folder and run:

```sh
cargo run --locked -p cirtam
```

To run the automated checks:

```sh
cargo test --locked
```

Use **Open** to load a circuit from `examples/`. Select **Run** or press **Space** to calculate its DC operating point.

## Project folder explanation

| Folder or file | Purpose |
|---|---|
| `apps/cirtam/` | Native desktop application: interface, drawing, input, and results display |
| `apps/cirtam-i/` | Placeholder crate for a possible future web edition |
| `crates/cirtam-core/` | Circuit model, topology, validation, solver, diagnostics, and `.crtm` persistence |
| `examples/` | Example circuit files |
| `spec/` | Specifications, including the `.crtm` save format |
| `agent/` | Project guidance and AI workflow documents |
| `Cargo.toml` | Rust workspace configuration |

## Version

*0.1 — Initial foundation Provides* a graphical circuit editor, basic linear DC components, DC operating-point solving, and `.crtm` circuit files. Future versions may add analyses and components after their behavior and file representation are defined.