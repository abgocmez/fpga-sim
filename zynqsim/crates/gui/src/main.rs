//! `zynqsim-gui`: the ZynqSim desktop front end.

mod app;
mod audio_out;
mod loader;
mod model;
mod source_view;
mod waves;

use app::{App, Opts};
use model::Model;
use sim_api::{Project, Ps, Severity, SimClient, SimConfig};
use std::path::{Path, PathBuf};
use std::time::Duration;

const USAGE: &str = "\
zynqsim-gui [--project FILE] [--play]

  --project FILE   open this project.toml
  --play           start running in real time

Options for automated checks:
  --toggle N,N     toggle these switches at start-up
  --run-ms N       advance the simulation by N ms before the window opens
  --exit-after-ms N  close the window after N ms
  --screenshot FILE  save a PNG of the window, then exit
  --uart TEXT      send TEXT (with \\r \\n escapes) to UART1, then run 5 ms; repeatable
  --mem ADDR       open the memory viewer at ADDR as a 64x64 image
  --mem-hex        with --mem: show the hex dump instead of the image
  --sections LIST  open or close side-panel sections, e.g. source=0,memory=1
  --view3d         start on the 3D board view
  --wave NAME      open the Waveforms tab with this signal shown; repeatable
  --project-window open the project window
";

fn die(msg: &str) -> ! {
    eprintln!("zynqsim-gui: {msg}");
    std::process::exit(2);
}

fn main() -> eframe::Result {
    let mut project: Option<PathBuf> = None;
    let mut play = false;
    let mut toggles: Vec<usize> = Vec::new();
    let mut run_ms = 0u64;
    let mut uart_lines: Vec<String> = Vec::new();
    let mut opts = Opts::default();
    // `--sections` overrides the remembered state for this run.
    let mut sections: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |what: &str| {
            args.next()
                .unwrap_or_else(|| die(&format!("{what} needs a value")))
        };
        match a.as_str() {
            "--project" => project = Some(PathBuf::from(value("--project"))),
            "--play" => play = true,
            "--toggle" => {
                for s in value("--toggle").split(',') {
                    toggles.push(
                        s.trim()
                            .parse()
                            .unwrap_or_else(|_| die("--toggle expects numbers")),
                    );
                }
            }
            "--run-ms" => {
                run_ms = value("--run-ms")
                    .parse()
                    .unwrap_or_else(|_| die("--run-ms expects a number"))
            }
            "--exit-after-ms" => {
                let ms: u64 = value("--exit-after-ms")
                    .parse()
                    .unwrap_or_else(|_| die("--exit-after-ms expects a number"));
                opts.exit_after = Some(Duration::from_millis(ms));
            }
            "--screenshot" => opts.screenshot = Some(PathBuf::from(value("--screenshot"))),
            "--uart" => uart_lines.push(value("--uart")),
            "--view3d" => opts.view_3d = true,
            "--wave" => opts.waves.push(value("--wave")),
            "--project-window" => opts.project_window = true,
            "--mem" => {
                let v = value("--mem");
                let t = v.trim();
                let n = match t.strip_prefix("0x") {
                    Some(h) => u64::from_str_radix(h, 16),
                    None => t.parse(),
                };
                opts.mem = Some(n.unwrap_or_else(|_| die("--mem expects an address")));
            }
            "--mem-hex" => opts.mem_hex = true,
            "--sections" => sections = Some(value("--sections").replace(',', "\n")),
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => die(&format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }

    opts.project = project.clone();
    let (client, cpu1) = match &project {
        Some(file) => open_project(file),
        None => (
            SimClient::spawn(SimConfig::default()).unwrap_or_else(|e| die(&e.to_string())),
            false,
        ),
    };
    let mut model = Model::new(client).unwrap_or_else(|e| die(&e.to_string()));
    model.set_cpu1_declared(cpu1);
    for i in toggles {
        model.toggle_switch(i);
    }
    if run_ms > 0 {
        model.run_for(run_ms * Ps::PER_MS);
    }
    for line in uart_lines {
        let text = line.replace("\\r", "\r").replace("\\n", "\n");
        model.send_uart(1, &text, false);
        model.run_for(5 * Ps::PER_MS);
    }
    if play {
        model.play();
    }
    model.refresh();
    if let Some(e) = &model.last_error {
        die(e);
    }

    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ZynqSim - Zybo Z7-20")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ZynqSim",
        native,
        Box::new(move |cc| {
            let mut app = App::new(model, opts);
            if let Some(saved) = cc.storage.and_then(|s| s.get_string(app::SECTIONS_KEY)) {
                app.restore_sections(&saved);
            }
            if let Some(list) = &sections {
                app.restore_sections(list);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Opens a project file; also says whether it declares a second core.
fn open_project(file: &Path) -> (SimClient, bool) {
    let p = Project::load(file).unwrap_or_else(|e| die(&e.message));
    let base = file.parent().unwrap_or(Path::new("."));
    let errors: Vec<String> = p
        .validate(base)
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}: {}", d.field, d.message))
        .collect();
    if !errors.is_empty() {
        die(&format!("project has errors:\n  {}", errors.join("\n  ")));
    }
    let cpu1 = p.cpu1.is_some();
    let engine = p.build_engine(base).unwrap_or_else(|e| die(&e.message));
    (SimClient::spawn_engine(engine), cpu1)
}
