//! A Luau plugin's settings pane (ADR 0029): its own page, which the host
//! draws from `settings.view` and sends as a tree, or a form drawn from
//! its schema. Both are view trees whose bound pieces (`toggle`,
//! `choice`, `field`) change a key of the plugin's settings, saved in
//! the scope the pane shows once the schema accepts the new value.

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui::{
    AnyElement,
    App,
    AppContext as _,
    Context,
    Entity,
    ParentElement,
    SharedString,
    Styled,
    div,
    prelude::*,
    rems,
};
use serde_json::{Value, json};
use tau_ui_kit::{
    components::{empty, field, switch},
    input::{InputEvent, TextInput},
    theme::{Design as _, Theme, Type, radius, sp},
};
use tau_ui_plugin::{Handle, PluginUi, ViewCx};

use crate::{
    Act,
    Declaration,
    LuauSettings,
    SettingsPage,
    settings::{self, FieldKind},
    ui::{LuauPluginsUi, draw_bound},
};

/// What the window keeps for the settings panes.
pub struct SettingsUi {
    handle: Handle,
    /// The pages the host drew, by plugin and the settings they show.
    pages: HashMap<(String, String), Result<Value, String>>,
    /// The pages asked for and not drawn yet.
    asked: HashSet<(String, String)>,
    /// Each field's input, by plugin, scope and key.
    fields: HashMap<String, Entity<TextInput>>,
    /// What a field's Enter sent: its id and text, saved as the pane
    /// draws next, when it has the settings at hand.
    submitted: Vec<(String, String)>,
    /// Why the last change did not hold, by plugin.
    rejected: HashMap<String, String>,
}

impl PluginUi for SettingsUi {
    fn new(handle: Handle, _cx: &mut Context<Self>) -> Self {
        Self {
            handle,
            pages: HashMap::new(),
            asked: HashSet::new(),
            fields: HashMap::new(),
            submitted: Vec::new(),
            rejected: HashMap::new(),
        }
    }
}

impl SettingsUi {
    /// Keeps a page the host drew, and draws the pane again.
    pub fn take_page(&mut self, page: SettingsPage, cx: &mut App) {
        let key = (page.plugin, page.settings.to_string());
        self.asked.remove(&key);
        self.pages.insert(key, page.page);
        self.handle.refresh(cx);
    }

    /// The input of field `id`, made with `text` the first time.
    fn field(
        &mut self,
        id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = self.fields.get(id) {
            return input.clone();
        }
        let input = cx.new(|cx| {
            let mut input = TextInput::new("", cx).keep_on_submit();
            input.set_text(text, cx);
            input
        });
        let (id_owned, handle) = (id.to_owned(), self.handle.clone());
        cx.subscribe(&input, move |ui, _, event: &InputEvent, cx| {
            let InputEvent::Submit(text) = event;
            ui.submitted.push((id_owned.clone(), text.clone()));
            handle.refresh(cx);
        })
        .detach();
        self.fields.insert(id.to_owned(), input.clone());
        input
    }
}

/// Saves a key of the settings shown.
pub type Save = Rc<dyn Fn(&str, Value, &mut App)>;

/// What bound pieces read and write: the settings shown, how to save a
/// key, and the inputs of the page's fields.
pub struct Bind {
    pub value: Value,
    pub save: Save,
    pub fields: HashMap<String, Entity<TextInput>>,
}

/// A field's id: its plugin, the scope shown and its key.
fn field_id(plugin: &str, scope: Option<&str>, key: &str) -> String {
    format!("{plugin}\u{0}{}\u{0}{key}", scope.unwrap_or_default())
}

/// Draws a bound piece of a settings page.
pub fn bound(piece: &Value, t: &Theme, bind: &Bind) -> AnyElement {
    let key = piece["key"].as_str().unwrap_or_default().to_owned();
    let current = settings::get_key(&bind.value, &key).clone();
    match piece["piece"].as_str().unwrap_or_default() {
        "toggle" => {
            let on = current.as_bool().unwrap_or(false);
            let save = bind.save.clone();
            div()
                .id(SharedString::from(format!("toggle-{key}")))
                .flex()
                .items_center()
                .gap(sp(3.))
                .cursor_pointer()
                .child(switch(on, t))
                .when_some(piece["label"].as_str(), |row, label| {
                    row.child(
                        div()
                            .typeset(Type::SMALL)
                            .text_color(t.text_soft)
                            .child(label.to_owned()),
                    )
                })
                .on_click(move |_, _, cx| save(&key, Value::Bool(!on), cx))
                .into_any_element()
        }
        "choice" => {
            let multi = piece["multi"].as_bool().unwrap_or(false);
            let picked: Vec<String> = if multi {
                current
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            } else {
                current.as_str().map(str::to_owned).into_iter().collect()
            };
            let options: Vec<String> = piece["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            div()
                .flex()
                .flex_wrap()
                .gap(sp(1.5))
                .children(options.iter().map(|option| {
                    let on = picked.contains(option);
                    let next = if multi {
                        // Keep the options' order.
                        let chosen: Vec<Value> = options
                            .iter()
                            .filter(|o| picked.contains(o) != (*o == option))
                            .map(|o| Value::String(o.clone()))
                            .collect();
                        Value::Array(chosen)
                    } else {
                        Value::String(option.clone())
                    };
                    let (save, key) = (bind.save.clone(), key.clone());
                    div()
                        .id(SharedString::from(format!(
                            "choice-{key}-{option}"
                        )))
                        .px(sp(3.))
                        .py(sp(1.5))
                        .rounded(radius::CONTROL)
                        .border_1()
                        .cursor_pointer()
                        .typeset(Type::CAPTION)
                        .when(on, |chip| {
                            chip.border_color(t.accent_border)
                                .bg(t.accent_soft)
                                .text_color(t.text)
                        })
                        .when(!on, |chip| {
                            chip.border_color(t.border).text_color(t.dim)
                        })
                        .child(option.clone())
                        .on_click(move |_, _, cx| save(&key, next.clone(), cx))
                }))
                .into_any_element()
        }
        "field" => match bind.fields.get(&key) {
            Some(input) => div()
                .flex()
                .flex_col()
                .gap(sp(1.))
                .child(field(input, piece["kind"] == "number", t))
                .child(
                    div()
                        .typeset(Type::MICRO)
                        .text_color(t.dim)
                        .child("Enter saves it."),
                )
                .into_any_element(),
            None => div().into_any_element(),
        },
        _ => div().into_any_element(),
    }
}

/// The keys of the fields in `tree`, with whether each is a number.
fn field_keys(tree: &Value, out: &mut Vec<(String, bool)>) {
    match tree {
        Value::Object(piece) => {
            if piece.get("piece").and_then(Value::as_str) == Some("field")
                && let Some(key) = piece.get("key").and_then(Value::as_str)
            {
                out.push((
                    key.to_owned(),
                    piece.get("kind").and_then(Value::as_str) == Some("number"),
                ));
            }
            piece.values().for_each(|value| field_keys(value, out));
        }
        Value::Array(items) => {
            items.iter().for_each(|item| field_keys(item, out))
        }
        _ => {}
    }
}

/// The form drawn from `declaration`'s schema: each setting with its
/// label and description, as bound pieces.
pub fn form(declaration: &Declaration) -> Value {
    let rows: Vec<Value> = settings::fields(&settings::schema_of(declaration))
        .into_iter()
        .map(|field| {
            let control = match field.kind {
                FieldKind::Switch => json!({ "piece": "toggle", "key": field.key, "label": Value::Null }),
                FieldKind::Choice { options, multi } => json!({
                    "piece": "choice", "key": field.key, "options": options, "multi": multi,
                }),
                FieldKind::Text => json!({ "piece": "field", "key": field.key, "kind": "text" }),
                FieldKind::Number { .. } => json!({ "piece": "field", "key": field.key, "kind": "number" }),
                FieldKind::Other => json!({ "piece": "text", "text": "Set in the plugin's file." }),
            };
            let mut head = vec![json!({ "piece": "text", "text": field.label })];
            if let Some(description) = field.description {
                head.push(json!({ "piece": "rich", "text": description }));
            }
            json!({ "piece": "stack", "children": [
                { "piece": "stack", "children": head },
                control,
            ] })
        })
        .collect();
    json!({ "piece": "stack", "children": rows })
}

/// The pane of the Luau plugin `entry`.
pub fn render(view: &mut ViewCx<'_, LuauPluginsUi>) -> AnyElement {
    let t = view.theme().clone();
    let Some(entry) = view.entry().map(str::to_owned) else {
        return empty("Each Luau plugin's settings are on its own row.", &t)
            .into_any_element();
    };
    let Some(declaration) = view
        .data
        .plugins
        .iter()
        .find(|plugin| plugin.name == entry)
        .and_then(|plugin| plugin.declaration.clone())
    else {
        return empty(format!("{entry} does not load."), &t).into_any_element();
    };
    if declaration.settings.is_none() {
        return empty(format!("{entry} has nothing to set."), &t)
            .into_any_element();
    }
    let scope = view.scope().map(str::to_owned);
    let all: LuauSettings = view.settings.clone();
    let (current, fallback) =
        settings::effective(&declaration, all.plugins.get(&entry));
    let schema = settings::schema_of(&declaration);

    // How a bound piece saves: the key set, checked, then the plugins'
    // settings in the scope shown.
    let save: Save = {
        let (ui, handle) = (view.ui.clone(), view.handle.clone());
        let (entry, scope, current, all) =
            (entry.clone(), scope.clone(), current.clone(), all.clone());
        Rc::new(move |key: &str, new: Value, cx: &mut App| {
            let next = settings::set_key(&current, key, new);
            match settings::check(&schema, &next) {
                Ok(()) => {
                    let mut all = all.clone();
                    all.plugins.insert(entry.clone(), next);
                    ui.update(cx, |ui, _| ui.rejected.remove(&entry));
                    handle.save_settings_in(scope.as_deref(), &all, cx);
                }
                Err(why) => {
                    ui.update(cx, |ui, _| {
                        ui.rejected.insert(entry.clone(), why)
                    });
                    handle.refresh(cx);
                }
            }
        })
    };

    // Fields whose Enter came since the last draw save now.
    let prefix = field_id(&entry, scope.as_deref(), "");
    let submitted: Vec<(String, String)> = view.ui.update(view.cx, |ui, _| {
        let (mine, rest) = ui
            .submitted
            .drain(..)
            .partition(|(id, _)| id.starts_with(&prefix));
        ui.submitted = rest;
        mine
    });

    // The tree: the plugin's own page, else the form.
    let tree = if declaration.hooks.settings_view {
        let key = (entry.clone(), current.to_string());
        let (page, ask) = view.ui.update(view.cx, |ui, _| {
            let page = ui.pages.get(&key).cloned();
            let ask = page.is_none() && ui.asked.insert(key.clone());
            (page, ask)
        });
        if ask {
            view.handle.act(
                Act::SettingsView {
                    plugin: entry.clone(),
                    settings: current.clone(),
                },
                view.cx,
            );
        }
        page
    } else {
        Some(Ok(form(&declaration)))
    };

    let mut keys = Vec::new();
    if let Some(Ok(tree)) = &tree {
        field_keys(tree, &mut keys);
    }
    let fields: HashMap<String, Entity<TextInput>> = keys
        .iter()
        .map(|(key, _)| {
            let id = field_id(&entry, scope.as_deref(), key);
            let text = match settings::get_key(&current, key) {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            let input =
                view.ui.update(view.cx, |ui, cx| ui.field(&id, text, cx));
            (key.clone(), input)
        })
        .collect();
    for (id, text) in submitted {
        let Some((key, number)) = keys
            .iter()
            .find(|(key, _)| field_id(&entry, scope.as_deref(), key) == id)
        else {
            continue;
        };
        let value = if *number {
            match text.trim().parse::<serde_json::Number>() {
                Ok(number) => Value::Number(number),
                Err(_) => {
                    let why = format!("`{key}` should be a number");
                    view.ui.update(view.cx, |ui, _| {
                        ui.rejected.insert(entry.clone(), why)
                    });
                    continue;
                }
            }
        } else {
            Value::String(text)
        };
        save(key, value, view.cx);
    }

    let rejected = view.ui.read(view.cx).rejected.get(&entry).cloned();
    let bind = Bind {
        value: current,
        save,
        fields,
    };
    let body = match tree {
        Some(Ok(tree)) => draw_bound(&tree, &t, &bind),
        Some(Err(error)) => div()
            .typeset(Type::CAPTION)
            .text_color(t.red)
            .child(format!("Its settings page failed: {error}"))
            .into_any_element(),
        None => div()
            .typeset(Type::CAPTION)
            .text_color(t.dim)
            .child("Drawing its settings page…")
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .p(sp(4.))
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.border)
        .bg(t.panel)
        .child(body)
        .children(rejected.into_iter().chain(fallback).map(|why| {
            div()
                .typeset(Type::CAPTION)
                .text_color(t.roles.live)
                .child(why)
        }))
        .child(div().h(rems(0.)))
        .into_any_element()
}
