//! Runs a command under a pseudo-terminal and shows it live in a
//! window, frozen once it exits:
//!
//! ```sh
//! cargo run -p tau-terminal --example run -- ls -la --color=always
//! ```
//!
//! With no command, it runs a short demo of colors and box drawing.
//! Scroll with the wheel, drag to select, and copy with Ctrl-Shift-C
//! (⌘C on macOS).

use gpui::{
    App,
    Bounds,
    Context,
    Entity,
    Window,
    WindowBounds,
    WindowOptions,
    div,
    prelude::*,
    px,
    rgb,
    size,
};
use tau_terminal::{Command, Event, TerminalView, ViewOptions, view};

const DEMO: &str = r#"
printf '\033[1;32m   Compiling\033[0m demo v0.1.0\n'
printf '\033[1;33mwarning\033[0;1m: unused variable\033[0m\n'
printf '\033[1;34m   -->\033[0m src/main.rs:4:9\n'
printf '┌──────┬──────┐\n│ \033[7m left \033[0m│ right│\n└──────┴──────┘\n'
printf '▁▂▃▄▅▆▇█ ░▒▓ 日本語\n'
for i in $(seq 1 60); do printf '\033[38;5;%dmline %d\033[0m\n' $((i + 16)) $i; done
printf '\033[1;32m    Finished\033[0m in 0.1s\n'
"#;

struct Example {
    terminal: Entity<TerminalView>,
}

impl Render for Example {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x141518))
            .p_4()
            .child(self.terminal.clone())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        args = vec!["sh".into(), "-c".into(), DEMO.into()];
    }
    // The runner's I/O runs on tokio; its events can be awaited anywhere.
    let runtime = tokio::runtime::Runtime::new()?;
    let _entered = runtime.enter();
    let mut command = Command::new(&args[0]);
    for arg in &args[1..] {
        command = command.arg(arg);
    }
    let mut run = command.spawn()?;

    gpui_platform::application().run(move |cx: &mut App| {
        view::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                let terminal = cx.new(|cx| {
                    TerminalView::new(
                        ViewOptions {
                            visible_rows: Some(36),
                            ..ViewOptions::default()
                        },
                        cx,
                    )
                    .expect("libghostty-vt creates a terminal")
                });
                let feed = terminal.downgrade();
                cx.spawn(async move |cx| {
                    while let Some(event) = run.next().await {
                        let fed = feed.update(cx, |terminal, cx| match event {
                            Event::Output(bytes) => terminal.write(&bytes, cx),
                            Event::Exit(_) => terminal.freeze(cx),
                            Event::Text(_) | Event::Screen(_) => Ok(()),
                        });
                        if !matches!(fed, Ok(Ok(()))) {
                            break;
                        }
                    }
                })
                .detach();
                cx.new(|_| Example { terminal })
            },
        );
        if opened.is_ok() {
            cx.activate(true);
        } else {
            cx.quit();
        }
    });
    Ok(())
}
