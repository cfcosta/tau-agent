//! A Luau plugin's settings pane (ADR 0029): its own page, which the host
//! draws from `settings.view` and sends as a tree, or a form drawn from
//! its schema. Both are view trees whose bound pieces (`toggle`,
//! `choice`, `field`) change a key of the plugin's settings, saved in
//! the scope the pane shows once the schema accepts the new value. The
//! pane binds them as the shared tree's switches, buttons and inputs
//! (ADR 0045).

use std::collections::{HashMap, HashSet};

use gpui::{AnyElement, App, Context, Entity};
use serde_json::{Value, json};
use tau_ui_plugin::{
    Handle,
    PluginUi,
    ViewCx,
    tree::{Kind, Node, Tone},
};

use crate::{
    Act,
    Declaration,
    LuauSettings,
    SettingsPage,
    settings::{self, FieldKind},
    ui::{LuauPluginsUi, rewrite},
};

/// What the window keeps for the settings panes: the pages the host
/// sends back, which [`UiPlugin::reply`](tau_ui_plugin::UiPlugin::reply)
/// hands it.
pub struct SettingsUi {
    handle: Handle,
    /// The pages the host drew, by plugin and the settings they show.
    pages: HashMap<(String, String), Result<Value, String>>,
    /// The pages asked for and not drawn yet.
    asked: HashSet<(String, String)>,
    /// Why the last change did not hold, by plugin.
    rejected: HashMap<String, String>,
}

impl PluginUi for SettingsUi {
    fn new(handle: Handle, _cx: &mut Context<Self>) -> Self {
        Self {
            handle,
            pages: HashMap::new(),
            asked: HashSet::new(),
            rejected: HashMap::new(),
        }
    }
}

impl SettingsUi {
    /// The way back to the interface.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Keeps a page the host drew, and draws the pane again.
    pub fn take_page(&mut self, page: SettingsPage, cx: &mut App) {
        let key = (page.plugin, page.settings.to_string());
        self.asked.remove(&key);
        self.pages.insert(key, page.page);
        self.handle.refresh(cx);
    }
}

/// A toggle's or a choice's action: sets `args.key` to `args.value`.
const SET: &str = "setting/set";

/// A text field's Enter, after which comes its key; `{ text }`.
const TEXT: &str = "setting/text:";

/// A number field's Enter, after which comes its key; `{ text }`.
const NUMBER: &str = "setting/number:";

/// How the pane's actions save: the key set, checked, then the plugins'
/// settings in the scope shown.
#[derive(Clone)]
pub struct Saver {
    ui: Entity<SettingsUi>,
    handle: Handle,
    entry: String,
    scope: Option<String>,
    current: Value,
    all: LuauSettings,
    schema: Value,
}

impl Saver {
    /// Carries out `action` when it is one of the pane's: whether it was.
    pub fn act(&self, action: &str, args: &Value, cx: &mut App) -> bool {
        if action == SET {
            if let Some(key) = args["key"].as_str() {
                self.save(key, args["value"].clone(), cx);
            }
            return true;
        }
        let text = args["text"].as_str().unwrap_or_default();
        if let Some(key) = action.strip_prefix(TEXT) {
            self.save(key, Value::String(text.to_owned()), cx);
            return true;
        }
        if let Some(key) = action.strip_prefix(NUMBER) {
            match text.trim().parse::<serde_json::Number>() {
                Ok(number) => self.save(key, Value::Number(number), cx),
                Err(_) => {
                    self.reject(format!("`{key}` should be a number"), cx)
                }
            }
            return true;
        }
        false
    }

    /// Saves `key` as `new`, when the schema takes it.
    fn save(&self, key: &str, new: Value, cx: &mut App) {
        let next = settings::set_key(&self.current, key, new);
        match settings::check(&self.schema, &next) {
            Ok(()) => {
                let mut all = self.all.clone();
                all.plugins.insert(self.entry.clone(), next);
                let entry = self.entry.clone();
                self.ui.update(cx, |ui, _| ui.rejected.remove(&entry));
                self.handle
                    .save_settings_in(self.scope.as_deref(), &all, cx);
            }
            Err(why) => self.reject(why, cx),
        }
    }

    /// Says why a change did not hold.
    fn reject(&self, why: String, cx: &mut App) {
        let entry = self.entry.clone();
        self.ui.update(cx, |ui, _| ui.rejected.insert(entry, why));
        self.handle.refresh(cx);
    }
}

/// The draft key's id of a field: its plugin, the scope shown and its
/// key.
fn field_id(plugin: &str, scope: Option<&str>, key: &str) -> String {
    format!("luau-settings/{plugin}/{}/{key}", scope.unwrap_or_default())
}

/// A bound piece of a settings page as the tree's own: a toggle a
/// switch, a choice a row of buttons, a field an input whose Enter
/// saves. Its ids start with `fields`.
pub fn bound(piece: &Value, current: &Value, fields: &str) -> Node {
    let key = piece["key"].as_str().unwrap_or_default().to_owned();
    let value = settings::get_key(current, &key).clone();
    let words = |name: &str| piece[name].as_str().map(str::to_owned);
    match piece["piece"].as_str().unwrap_or_default() {
        "toggle" => {
            let on = value.as_bool().unwrap_or(false);
            Node::Switch {
                label: words("label").unwrap_or_default(),
                detail: words("detail"),
                on,
                action: SET.into(),
                args: json!({ "key": key, "value": !on }),
            }
        }
        "choice" => {
            let multi = piece["multi"].as_bool().unwrap_or(false);
            let picked: Vec<String> = if multi {
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            } else {
                value.as_str().map(str::to_owned).into_iter().collect()
            };
            let options: Vec<String> = piece["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            Node::row(options.iter().map(|option| {
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
                Node::Button {
                    label: option.clone(),
                    action: SET.into(),
                    args: json!({ "key": key, "value": next }),
                    kind: if on { Kind::Primary } else { Kind::Secondary },
                    icon: None,
                }
            }))
        }
        "field" => {
            let text = match &value {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            let submit = if piece["kind"] == "number" {
                NUMBER
            } else {
                TEXT
            };
            Node::stack([
                Node::Input {
                    id: format!("{fields}{key}"),
                    placeholder: Some(text),
                    multiline: false,
                    submit: Some(format!("{submit}{key}")),
                    keep: true,
                    secret: false,
                },
                Node::text("Enter saves it."),
            ])
        }
        _ => Node::read(piece),
    }
}

/// The form drawn from `declaration`'s schema: each setting with its
/// label and description, as bound pieces.
pub fn form(declaration: &Declaration) -> Value {
    let rows: Vec<Value> = settings::fields(&settings::schema_of(declaration))
        .into_iter()
        .map(|field| {
            let control = match field.kind {
                // A switch's row has its label and small print.
                FieldKind::Switch => {
                    return json!({
                        "piece": "toggle", "key": field.key,
                        "label": field.label, "detail": field.description,
                    });
                }
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

/// The pane of the Luau plugin the view is of, as a tree, and how its
/// actions save, when it has settings to save.
pub fn pane(view: &mut ViewCx<'_, LuauPluginsUi>) -> (Node, Option<Saver>) {
    let Some(entry) = view.entry().map(str::to_owned) else {
        let empty = Node::Empty {
            text: "Each Luau plugin's settings are on its own row.".into(),
        };
        return (empty, None);
    };
    let Some(declaration) = view
        .data
        .plugins
        .iter()
        .find(|plugin| plugin.name == entry)
        .and_then(|plugin| plugin.declaration.clone())
    else {
        let empty = Node::Empty {
            text: format!("{entry} does not load."),
        };
        return (empty, None);
    };
    if declaration.settings.is_none() {
        let empty = Node::Empty {
            text: format!("{entry} has nothing to set."),
        };
        return (empty, None);
    }
    let scope = view.scope().map(str::to_owned);
    let all: LuauSettings = view.settings.clone();
    let (current, fallback) =
        settings::effective(&declaration, all.plugins.get(&entry));

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

    let fields = field_id(&entry, scope.as_deref(), "");
    let body = match tree {
        Some(Ok(tree)) => rewrite(Node::read(&tree), &mut |node| {
            if node.is_setting() {
                bound(&node.value(), &current, &fields)
            } else {
                node
            }
        }),
        Some(Err(error)) => Node::Status {
            text: format!("Its settings page failed: {error}"),
            detail: None,
            tone: Tone::Bad,
        },
        None => Node::text("Drawing its settings page…"),
    };
    let rejected = view.ui.read(view.cx).rejected.get(&entry).cloned();
    let mut children = vec![body];
    children.extend(rejected.into_iter().chain(fallback).map(|why| {
        Node::Status {
            text: why,
            detail: None,
            tone: Tone::Warn,
        }
    }));
    let saver = Saver {
        ui: view.ui.clone(),
        handle: view.handle.clone(),
        entry,
        scope,
        current,
        all,
        schema: settings::schema_of(&declaration),
    };
    (Node::Card { children }, Some(saver))
}

/// The pane of the Luau plugin `entry`.
pub fn render(view: &mut ViewCx<'_, LuauPluginsUi>) -> AnyElement {
    let (tree, saver) = pane(view);
    let id = format!("luau-settings-{}", view.entry().unwrap_or_default());
    view.tree(&id, &tree, move |action, args, cx| {
        if let Some(saver) = &saver {
            saver.act(action, args, cx);
        }
    })
}
