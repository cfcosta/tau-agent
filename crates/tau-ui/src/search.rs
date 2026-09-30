//! The search palette (Ctrl K): conversations in every repository,
//! the repositories, and what the app can do, filtered as the user
//! types. Enter opens the first match; any match opens on a click.

use gpui::{
    AnyElement,
    Context,
    Focusable as _,
    SharedString,
    Window,
    div,
    prelude::*,
    px,
};
use tau_agent::tool::RunId;

use crate::{
    assets::Icon,
    route::Route,
    theme::{IconSize, Theme, Type, radius, sp},
    ui,
    view::Origin,
    workspace::Workspace,
};

/// How many matches the palette lists.
const SHOWN: usize = 8;

/// What a match does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Run(RunId),
    Repo(String),
    NewRunIn(String),
    Screen(Route),
    AddRepo,
}

/// One match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub label: String,
    /// Where it is, or what it is.
    pub detail: String,
    pub glyph: Icon,
    pub pick: Pick,
}

impl Workspace {
    /// What `query` matches, best first: conversations, then
    /// repositories, then things to do. Every word must match.
    pub fn search_hits(&self, query: &str) -> Vec<Hit> {
        let query = query.trim().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let matches = |text: &str| {
            let text = text.to_lowercase();
            words.iter().all(|word| text.contains(word))
        };
        let mut hits = Vec::new();
        if !words.is_empty() {
            for run in &self.runs {
                let repo = self.repo_of(run);
                if !matches(&format!("{} {repo} {}", run.title, run.agent)) {
                    continue;
                }
                let kind = match run.origin {
                    Origin::Fork { .. } => "fork",
                    Origin::SubAgent { .. } => "sub-agent",
                    Origin::Root => "run",
                };
                let closed = if self.is_closed(&run.id) {
                    " · closed"
                } else {
                    ""
                };
                hits.push(Hit {
                    label: run.title.clone(),
                    detail: format!("{kind} in {repo}{closed}"),
                    glyph: match run.origin {
                        Origin::Fork { .. } => Icon::Fork,
                        _ => Icon::Chat,
                    },
                    pick: Pick::Run(run.id.clone()),
                });
            }
        }
        for repo in &self.catalog.repos {
            if words.is_empty() || matches(&repo.name) {
                hits.push(Hit {
                    label: repo.name.clone(),
                    detail: "repository".into(),
                    glyph: Icon::Repo,
                    pick: Pick::Repo(repo.name.clone()),
                });
            }
        }
        let mut actions: Vec<Hit> = self
            .catalog
            .repos
            .iter()
            .flat_map(|repo| {
                let name = repo.name.clone();
                [
                    Hit {
                        label: format!("New run in {name}"),
                        detail: "action".into(),
                        glyph: Icon::Plus,
                        pick: Pick::NewRunIn(name.clone()),
                    },
                    Hit {
                        label: format!("{name} memory"),
                        detail: "screen".into(),
                        glyph: Icon::Memory,
                        pick: Pick::Screen(Route::Memory {
                            repo: name.clone(),
                            note: None,
                        }),
                    },
                    Hit {
                        label: format!("{name} constitution"),
                        detail: "screen".into(),
                        glyph: Icon::Blocked,
                        pick: Pick::Screen(Route::Constitution {
                            repo: name,
                            rule: None,
                        }),
                    },
                ]
            })
            .collect();
        actions.extend([
            screen("History", Icon::History, Route::History),
            screen("Plugins", Icon::Plug, Route::Plugins),
            screen("Models and accounts", Icon::Settings, Route::Models),
            Hit {
                label: "Add a repository".into(),
                detail: "action".into(),
                glyph: Icon::Folder,
                pick: Pick::AddRepo,
            },
        ]);
        hits.extend(
            actions
                .into_iter()
                .filter(|hit| words.is_empty() || matches(&hit.label)),
        );
        hits.truncate(SHOWN);
        hits
    }

    /// Opens the palette, empty.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.searching = true;
        self.search.update(cx, |input, cx| input.clear(cx));
        self.search.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Opens the palette on `query`, without moving the focus: for
    /// looking at it.
    pub fn show_search(&mut self, query: &str, cx: &mut Context<Self>) {
        self.searching = true;
        self.search
            .update(cx, |input, cx| input.set_text(query.to_owned(), cx));
        cx.notify();
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    pub fn close_search(&mut self, cx: &mut Context<Self>) {
        self.searching = false;
        cx.notify();
    }

    /// Does what a match does, and closes the palette.
    pub fn pick(
        &mut self,
        pick: Pick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.searching = false;
        match pick {
            Pick::Run(run) => self.navigate(Route::Run(run), cx),
            Pick::Repo(repo) => {
                if !self.is_repo_open(&repo) {
                    self.toggle_repo_open(&repo, cx);
                }
                self.repo = Some(repo);
            }
            Pick::NewRunIn(repo) => self.new_run_in(&repo, window, cx),
            Pick::Screen(route) => self.navigate(route, cx),
            Pick::AddRepo => self.pick_github_repos(cx),
        }
        cx.notify();
    }

    /// Opens the first match of what was typed.
    pub(crate) fn pick_first(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.search.read(cx).text().to_owned();
        if let Some(hit) = self.search_hits(&query).into_iter().next() {
            self.pick(hit.pick, window, cx);
        }
    }

    /// The palette, over the app.
    pub(crate) fn search_view(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.search.read(cx).text().to_owned();
        let hits = self.search_hits(&query);
        let rows: Vec<AnyElement> = hits
            .into_iter()
            .enumerate()
            .map(|(n, hit)| {
                let pick = hit.pick.clone();
                div()
                    .id(SharedString::from(format!("hit-{n}")))
                    .flex()
                    .items_center()
                    .gap(sp(2.5))
                    .h(px(36.))
                    .px(sp(3.))
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .when(n == 0, |row| row.bg(t.selected))
                    .hover(|style| style.bg(t.selected))
                    .child(ui::icon(hit.glyph, IconSize::BASE, t.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(hit.label),
                    )
                    .child(ui::text(hit.detail, Type::CAPTION, t.dim))
                    .on_click(cx.listener(move |ws, _, window, cx| {
                        ws.pick(pick.clone(), window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(sp(20.))
            .px(sp(4.))
            .bg(t.scrim)
            .occlude()
            .child(
                div()
                    .id("search")
                    .w(px(560.))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap(sp(1.))
                    .p(sp(2.))
                    .bg(t.panel)
                    .border_1()
                    .border_color(t.border_strong)
                    .rounded(radius::CARD)
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(2.))
                            .px(sp(2.))
                            .child(ui::icon(
                                Icon::Search,
                                IconSize::BASE,
                                t.dim,
                            ))
                            .child(div().flex_1().child(ui::field(
                                &self.search,
                                false,
                                t,
                            ))),
                    )
                    .children(rows)
                    .when(empty, |list| {
                        list.child(
                            div()
                                .px(sp(3.))
                                .py(sp(3.))
                                .text_color(t.muted)
                                .child("Nothing matches."),
                        )
                    })
                    .on_mouse_down_out(
                        cx.listener(|ws, _, _, cx| ws.close_search(cx)),
                    ),
            )
            .into_any_element()
    }
}

fn screen(label: &str, glyph: Icon, route: Route) -> Hit {
    Hit {
        label: label.into(),
        detail: "screen".into(),
        glyph,
        pick: Pick::Screen(route),
    }
}
