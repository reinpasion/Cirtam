mod app;
mod canvas;
#[cfg(test)]
mod canvas_tests;
mod palette;
mod resistance;
mod simulation;
mod simulation_ui;
mod telemetry;
mod wiring;

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Cirtam 0.1",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1180.0, 760.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            let theme = match theme_path()
                .and_then(|path| {
                    std::fs::read_to_string(&path).ok().or_else(|| {
                        let legacy = path.parent()?.parent()?.join("Cirmat").join("theme");
                        std::fs::read_to_string(legacy).ok()
                    })
                })
                .as_deref()
            {
                Some("light") => eframe::egui::Theme::Light,
                _ => eframe::egui::Theme::Dark,
            };
            cc.egui_ctx.set_theme(theme);
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(app::Editor::default()))
        }),
    )
}

fn install_fonts(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};

    let mut fonts = FontDefinitions::default();
    for (name, family, bytes) in [
        (
            "IBM Plex Sans",
            FontFamily::Proportional,
            include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf").as_slice(),
        ),
        (
            "IBM Plex Mono",
            FontFamily::Monospace,
            include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), FontData::from_static(bytes).into());
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.into());
    }
    ctx.set_fonts(fonts);
}

fn theme_path() -> Option<std::path::PathBuf> {
    let dir = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            })
    };
    dir.map(|dir| dir.join("Cirtam").join("theme"))
}

fn save_theme(dark: bool) -> std::io::Result<()> {
    let Some(path) = theme_path() else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, if dark { "dark" } else { "light" })
}
