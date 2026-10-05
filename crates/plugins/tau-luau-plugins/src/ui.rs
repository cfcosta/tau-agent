//! tau-luau-plugins' UI (ADR 0017, 0027): the records folded into each
//! run's state, the view trees Luau plugins return drawn with tau-ui-kit's
//! pieces, the run's status line, the cards of the plugins' own tools,
//! and a badge on calls a plugin flagged.
//!
//! The drawing reads JSON only, so the phone, which runs no Luau, draws
//! what the computer does.

use std::collections::BTreeMap;

use gpui::{
    AnyElement,
    IntoElement,
    ParentElement,
    Styled,
    div,
    prelude::*,
    relative,
    rems,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_kit::{
    components::{badge, code_block, dot, mono},
    prose::rich,
    theme::{Design as _, Theme, Tone, Type, radius, sp},
};
use tau_ui_plugin::{
    Fold,
    HostCx,
    Manifest,
    PluginInfo,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    points::{self, AtCard, CardView},
};

use crate::{NAME, Record};

/// How deep a view tree may nest, and how many pieces a list or a row
/// may hold, before the rest is left out.
const MAX_DEPTH: usize = 12;
const MAX_CHILDREN: usize = 200;
/// Log lines a run keeps per plugin, the newest.
const MAX_LOGS: usize = 200;

/// tau-luau-plugins with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct LuauPluginsUi;

/// What a run's Luau plugins left, by plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub plugins: BTreeMap<String, PluginState>,
}

/// One plugin's part in a run, as its records leave it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginState {
    /// What its `view` drew last: `{ status?, note?, card?, page? }`.
    pub view: Value,
    pub state: Value,
    /// Hooks that failed, and why, oldest first.
    pub errors: Vec<(String, String)>,
    /// Why it is off for the rest of the run, once it is.
    pub off: Option<String>,
    /// Calls it flagged, and why, by call id.
    pub flags: BTreeMap<String, String>,
    pub logs: Vec<String>,
}

impl Fold for State {
    type Record = Record;

    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
        match record {
            Record::State { plugin, state } => self.of(plugin).state = state,
            Record::View { plugin, view } => self.of(plugin).view = view,
            Record::Log { plugin, lines } => {
                let logs = &mut self.of(plugin).logs;
                logs.extend(lines);
                let over = logs.len().saturating_sub(MAX_LOGS);
                logs.drain(..over);
            }
            Record::Error {
                plugin,
                hook,
                error,
            } => self.of(plugin).errors.push((hook, error)),
            Record::Flag {
                plugin,
                call_id,
                reason,
            } => {
                self.of(plugin).flags.insert(call_id, reason);
            }
            Record::Off { plugin, why } => self.of(plugin).off = Some(why),
        }
    }
}

impl State {
    fn of(&mut self, plugin: String) -> &mut PluginState {
        self.plugins.entry(plugin).or_default()
    }

    /// The run's line in the plugin list: each plugin's status, or that
    /// it is off or failing.
    pub fn status(&self) -> Option<String> {
        let lines: Vec<String> = self
            .plugins
            .iter()
            .map(|(name, plugin)| {
                if let Some(why) = &plugin.off {
                    return format!("{name}: off · {why}");
                }
                let status = &plugin.view["status"];
                let text = status["text"].as_str().unwrap_or("on");
                let mut line = format!("{name}: {text}");
                if let Some(detail) = status["detail"].as_str() {
                    line.push_str(&format!(" · {detail}"));
                }
                if !plugin.errors.is_empty() {
                    line.push_str(&format!(
                        " · {} failed",
                        plugin.errors.len()
                    ));
                }
                line
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("; "))
    }
}

/// A piece's tone, as the theme has it.
fn tone(of: &Value) -> Tone {
    match of.as_str() {
        Some("good") => Tone::Good,
        Some("warn") => Tone::Warn,
        Some("bad") => Tone::Danger,
        Some("info") => Tone::Info,
        _ => Tone::Quiet,
    }
}

/// Draws a view tree a Luau plugin returned. A piece it does not know,
/// or past [`MAX_DEPTH`], draws as a red box that says so.
pub fn draw(piece: &Value, t: &Theme) -> AnyElement {
    draw_at(piece, t, 0)
}

fn text(piece: &Value, key: &str) -> String {
    piece[key].as_str().unwrap_or_default().to_owned()
}

fn children(
    piece: &Value,
    key: &str,
    t: &Theme,
    depth: usize,
) -> Vec<AnyElement> {
    piece[key]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_CHILDREN)
        .map(|child| draw_at(child, t, depth + 1))
        .collect()
}

fn unknown(what: String, t: &Theme) -> AnyElement {
    div()
        .px(sp(2.))
        .py(sp(1.))
        .rounded(radius::SMALL)
        .border_1()
        .border_color(t.red_border)
        .typeset(Type::CAPTION)
        .text_color(t.red)
        .child(what)
        .into_any_element()
}

fn draw_at(piece: &Value, t: &Theme, depth: usize) -> AnyElement {
    if depth > MAX_DEPTH {
        return unknown("the view nests too deep".into(), t);
    }
    if let Some(plain) = piece.as_str() {
        return div()
            .typeset(Type::SMALL)
            .text_color(t.text_soft)
            .child(plain.to_owned())
            .into_any_element();
    }
    match piece["piece"].as_str().unwrap_or_default() {
        "text" => div()
            .typeset(Type::SMALL)
            .text_color(t.text_soft)
            .child(text(piece, "text"))
            .into_any_element(),
        "rich" => div()
            .typeset(Type::SMALL)
            .child(rich(&text(piece, "text"), t.text_soft, t))
            .into_any_element(),
        "mono" => mono(text(piece, "text"), Type::CAPTION, t.text_soft)
            .into_any_element(),
        "badge" => {
            let color = t.tone(tone(&piece["tone"]));
            badge(text(piece, "text"), color, color.opacity(0.4))
                .into_any_element()
        }
        "status" => {
            let color = t.tone(tone(&piece["tone"]));
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(dot(color, 7.))
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .child(text(piece, "text")),
                )
                .when_some(piece["detail"].as_str(), |row, detail| {
                    row.child(mono(detail.to_owned(), Type::CAPTION, t.dim))
                })
                .into_any_element()
        }
        "rows" => div()
            .flex()
            .flex_col()
            .gap(sp(1.))
            .children(
                piece["rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(MAX_CHILDREN)
                    .map(|row| {
                        div()
                            .flex()
                            .gap(sp(3.))
                            .child(
                                mono(text(row, "key"), Type::CAPTION, t.dim)
                                    .w(rems(8.))
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .typeset(Type::SMALL)
                                    .text_color(t.text_soft)
                                    .child(text(row, "value")),
                            )
                    }),
            )
            .into_any_element(),
        "list" => div()
            .flex()
            .flex_col()
            .gap(sp(1.))
            .children(children(piece, "items", t, depth).into_iter().map(
                |item| {
                    div()
                        .flex()
                        .gap(sp(2.))
                        .child(div().text_color(t.dim).child("•"))
                        .child(div().flex_1().min_w(rems(0.)).child(item))
                },
            ))
            .into_any_element(),
        "progress" => {
            let share =
                piece["share"].as_f64().unwrap_or(0.).clamp(0., 1.) as f32;
            div()
                .flex()
                .flex_col()
                .gap(sp(1.))
                .when_some(piece["label"].as_str(), |column, label| {
                    column.child(mono(label.to_owned(), Type::CAPTION, t.dim))
                })
                .child(
                    div()
                        .w_full()
                        .h(rems(0.375))
                        .rounded(radius::SMALL)
                        .bg(t.raised)
                        .child(
                            div()
                                .h_full()
                                .w(relative(share))
                                .rounded(radius::SMALL)
                                .bg(t.accent),
                        ),
                )
                .into_any_element()
        }
        "code" => code_block(piece["lang"].as_str(), &text(piece, "text"), t)
            .into_any_element(),
        "stack" => div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .children(children(piece, "children", t, depth))
            .into_any_element(),
        "row" => div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(2.))
            .children(children(piece, "children", t, depth))
            .into_any_element(),
        // Buttons send their action once actions arrive (ADR 0027, phase 2).
        "button" => badge(text(piece, "label"), t.muted, t.border_strong)
            .into_any_element(),
        other => unknown(format!("unknown piece `{other}`"), t),
    }
}

impl UiPlugin for LuauPluginsUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    #[cfg(feature = "host")]
    type Host = crate::registry::Registry;
    #[cfg(not(feature = "host"))]
    type Host = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    async fn agent_plugins(
        &self,
        host: &Self::Host,
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        #[cfg(feature = "host")]
        {
            let active = host.active().await;
            if active.is_empty() {
                return Ok(Vec::new());
            }
            let kind = match run.kind {
                tau_ui_plugin::RunKind::Main => "main",
                tau_ui_plugin::RunKind::Chat => "chat",
                tau_ui_plugin::RunKind::SubAgent => "sub_agent",
            };
            let info = serde_json::json!({
                "kind": kind,
                "repo": run.repo.name,
                "model": run.model,
            });
            let jev = run
                .services
                .get::<std::sync::Arc<dyn tau_jev::Jev>>()
                .cloned();
            Ok(vec![Box::new(crate::agent::LuauPlugins::new(
                active, info, jev,
            ))])
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = (host, run);
            Ok(Vec::new())
        }
    }

    async fn catalog(
        &self,
        host: &Self::Host,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        #[cfg(feature = "host")]
        let description = {
            let names: Vec<String> = host
                .active()
                .await
                .iter()
                .map(|active| active.loaded.declaration.name.clone())
                .collect();
            if names.is_empty() {
                "Plugins written in Luau: none yet".to_owned()
            } else {
                format!("Plugins written in Luau: {}", names.join(", "))
            }
        };
        #[cfg(not(feature = "host"))]
        let description = {
            let _ = host;
            "Plugins written in Luau".to_owned()
        };
        PluginInfo {
            description,
            seams: vec![
                Seam::Tools,
                Seam::BeforeTool,
                Seam::BeforeStop,
                Seam::Finish,
            ],
            page: None,
            ..Default::default()
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .status(State::status)
            .contribute(points::CARD, |at: &AtCard, view| {
                let details = at.data.result.as_ref()?.details.as_ref()?;
                details.get("plugin")?.as_str()?;
                let card =
                    details.get("card").filter(|card| !card.is_null())?;
                let t = view.theme().clone();
                Some(CardView {
                    body: Some(
                        div()
                            .px(sp(3.5))
                            .py(sp(2.5))
                            .child(draw(card, &t))
                            .into_any_element(),
                    ),
                    ..CardView::default()
                })
            })
            .contribute(points::CARD_BADGE, |at: &AtCard, view| {
                let state: &State = view.state?;
                let (plugin, reason) =
                    state.plugins.iter().find_map(|(name, plugin)| {
                        plugin
                            .flags
                            .get(&at.call_id)
                            .map(|reason| (name.clone(), reason.clone()))
                    })?;
                let t = view.theme().clone();
                Some(
                    badge(
                        format!("flagged by {plugin}: {reason}"),
                        t.tone(Tone::Warn),
                        t.tone(Tone::Warn).opacity(0.4),
                    )
                    .into_any_element(),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Records fold into each plugin's part: its state and view, its log
    /// (the newest lines), its errors, its flags, and that it is off.
    #[test]
    fn records_fold_by_plugin() {
        let mut state = State::default();
        let p = || "p".to_owned();
        for record in [
            Record::State {
                plugin: p(),
                state: json!({ "turns": 2 }),
            },
            Record::View {
                plugin: p(),
                view: json!({ "status": { "text": "on", "detail": "2 turns" } }),
            },
            Record::Log {
                plugin: p(),
                lines: (0..250).map(|n| n.to_string()).collect(),
            },
            Record::Error {
                plugin: p(),
                hook: "turn_end".into(),
                error: "no luck".into(),
            },
            Record::Flag {
                plugin: p(),
                call_id: "c1".into(),
                reason: "big".into(),
            },
        ] {
            state
                .apply(record, &mut tau_ui_plugin::testing::FakeRun::default());
        }
        let part = &state.plugins["p"];
        assert_eq!(part.state, json!({ "turns": 2 }));
        assert_eq!(part.logs.len(), MAX_LOGS);
        assert_eq!(part.logs.first().map(String::as_str), Some("50"));
        assert_eq!(part.flags["c1"], "big");
        assert_eq!(
            state.status().as_deref(),
            Some("p: on · 2 turns · 1 failed")
        );
        state.apply(
            Record::Off {
                plugin: p(),
                why: "3 hooks failed".into(),
            },
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
        assert_eq!(state.status().as_deref(), Some("p: off · 3 hooks failed"));
    }
}
