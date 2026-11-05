//! A Luau plugin's settings pane (ADR 0029): its own page, which the host
//! draws from `settings.view` and sends as a tree, or a form drawn from
//! its schema. Both are view trees whose bound pieces (`toggle`,
//! `choice`, `field`) change a key of the plugin's settings, saved in
//! the scope the pane shows once the schema accepts the new value. The
//! pane binds them as the shared tree's switches, buttons and inputs
//! (ADR 0045). The host draws the plugin's own page, bound, and the
//! interface fetches it as a tree of the pane's.

use serde_json::{Value, json};
use tau_ui_plugin::{
    Handle,
    ViewCx,
    tree::{Kind, Node, Tone},
};

use crate::{
    Act,
    Declaration,
    LuauSettings,
    settings::{self, FieldKind},
    ui::{LuauPluginsUi, rewrite},
};

/// What the UI state keys of why a plugin's last change did not hold
/// start with, before the plugin's name.
const REJECTED: &str = "rejected/";

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
    handle: Handle,
    entry: String,
    scope: Option<String>,
    current: Value,
    all: LuauSettings,
    schema: Value,
}

impl Saver {
    /// Carries out `action` when it is one of the pane's: whether it was.
    pub fn act(&self, action: &str, args: &Value) -> bool {
        if action == SET {
            if let Some(key) = args["key"].as_str() {
                self.save(key, args["value"].clone());
            }
            return true;
        }
        let text = args["text"].as_str().unwrap_or_default();
        if let Some(key) = action.strip_prefix(TEXT) {
            self.save(key, Value::String(text.to_owned()));
            return true;
        }
        if let Some(key) = action.strip_prefix(NUMBER) {
            match text.trim().parse::<serde_json::Number>() {
                Ok(number) => self.save(key, Value::Number(number)),
                Err(_) => self.reject(format!("`{key}` should be a number")),
            }
            return true;
        }
        false
    }

    /// Saves `key` as `new`, when the schema takes it.
    fn save(&self, key: &str, new: Value) {
        let next = settings::set_key(&self.current, key, new);
        match settings::check(&self.schema, &next) {
            Ok(()) => {
                let mut all = self.all.clone();
                all.plugins.insert(self.entry.clone(), next);
                let rejected = format!("{REJECTED}{}", self.entry);
                self.handle.set_ui(rejected, None);
                self.handle.save_settings_in(self.scope.as_deref(), &all);
            }
            Err(why) => self.reject(why),
        }
    }

    /// Says why a change did not hold.
    fn reject(&self, why: String) {
        let rejected = format!("{REJECTED}{}", self.entry);
        self.handle.set_ui(rejected, Some(Value::String(why)));
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
                    focus: None,
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
    let secrets = secrets_card(&entry, &declaration);
    if declaration.settings.is_none() {
        let card = secrets.unwrap_or_else(|| Node::Empty {
            text: format!("{entry} has nothing to set."),
        });
        return (card, None);
    }
    let scope = view.scope().map(str::to_owned);
    let all: LuauSettings = view.settings.clone();
    let (current, fallback) =
        settings::effective(&declaration, all.plugins.get(&entry));

    // The tree: the plugin's own page, which the host draws of the
    // settings shown, else the form.
    let fields = field_id(&entry, scope.as_deref(), "");
    let body = if declaration.hooks.settings_view {
        Node::Fetched {
            key: format!("settings-view/{fields}/{current}"),
            fetch: json!(Act::SettingsView {
                plugin: entry.clone(),
                settings: current.clone(),
                fields: fields.clone(),
            }),
        }
    } else {
        Node::read(
            &settings_view(Ok(form(&declaration)), &current, &fields)["tree"],
        )
    };
    let rejected = view
        .ui_value(&format!("{REJECTED}{entry}"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut children = vec![body];
    children.extend(rejected.into_iter().chain(fallback).map(|why| {
        Node::Status {
            text: why,
            detail: None,
            tone: Tone::Warn,
        }
    }));
    children.extend(secrets);
    let saver = Saver {
        handle: view.handle.clone(),
        entry,
        scope,
        current,
        all,
        schema: settings::schema_of(&declaration),
    };
    (Node::Card { children }, Some(saver))
}

/// The answer to an [`Act::SettingsView`]: the plugin's settings page
/// `page`, its bound pieces as the pane's of `current` with their inputs'
/// ids starting with `fields`, or why it failed; as a fetched tree.
pub fn settings_view(
    page: Result<Value, String>,
    current: &Value,
    fields: &str,
) -> Value {
    let tree = match page {
        Ok(tree) => rewrite(Node::read(&tree), &mut |node| {
            if node.is_setting() {
                bound(&node.value(), current, fields)
            } else {
                node
            }
        }),
        Err(error) => Node::Status {
            text: format!("Its settings page failed: {error}"),
            detail: None,
            tone: Tone::Bad,
        },
    };
    json!({ "tree": tree.value() })
}

/// The action of a secret's input, with its name after it.
const SET_SECRET: &str = "set-secret/";

/// A field for each secret `declaration` names, which keeps what is typed
/// as the secret once Enter is pressed: never shown, never in the
/// interface's state.
fn secrets_card(entry: &str, declaration: &Declaration) -> Option<Node> {
    if declaration.uses.secrets.is_empty() {
        return None;
    }
    let mut children = vec![Node::Heading {
        text: "Secrets".into(),
    }];
    for name in &declaration.uses.secrets {
        children.push(Node::Rich {
            text: format!("`{name}`, for its requests to {}", {
                let hosts = &declaration.uses.network;
                if hosts.is_empty() {
                    "no host".to_owned()
                } else {
                    hosts.join(", ")
                }
            }),
        });
        children.push(Node::Input {
            id: format!("secret/{entry}/{name}"),
            placeholder: Some("Type a new value, then press Enter".into()),
            multiline: false,
            submit: Some(format!("{SET_SECRET}{name}")),
            keep: false,
            secret: true,
            focus: None,
        });
    }
    Some(Node::Card { children })
}

/// The pane of the Luau plugin `entry`.
pub fn render(view: &mut ViewCx<'_, LuauPluginsUi>) -> Node {
    pane(view).0
}

/// Carries out `action` when it is one of the pane's, as the pane the
/// view is of saves: whether it was.
pub fn act(
    action: &str,
    args: &Value,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) -> bool {
    if let (Some(name), Some(plugin)) =
        (action.strip_prefix(SET_SECRET), view.entry())
    {
        if let Some(value) = args["text"].as_str().filter(|v| !v.is_empty()) {
            view.handle.act(Act::SetSecret {
                plugin: plugin.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            });
        }
        return true;
    }
    pane(view).1.is_some_and(|saver| saver.act(action, args))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The host's page comes back with its bound pieces as the pane's,
    /// of the settings it was drawn of: a toggle a switch that flips its
    /// key, a field an input under the pane's ids; a failure says why.
    #[test]
    fn a_settings_page_comes_back_bound() {
        let page = json!({ "piece": "stack", "children": [
            { "piece": "toggle", "key": "loud", "label": "Loud" },
            { "piece": "field", "key": "name", "kind": "text" },
        ] });
        let current = json!({ "loud": true, "name": "Ada" });
        let answer = settings_view(Ok(page), &current, "fields/");
        let Node::Stack { children, .. } = Node::read(&answer["tree"]) else {
            panic!("not a stack: {answer}");
        };
        assert!(matches!(
            &children[0],
            Node::Switch { on: true, args, .. } if args == &json!({ "key": "loud", "value": false })
        ));
        let Node::Stack {
            children: field, ..
        } = &children[1]
        else {
            panic!("not a field: {:?}", children[1]);
        };
        assert!(matches!(
            &field[0],
            Node::Input { id, placeholder: Some(text), .. }
                if id == "fields/name" && text == "Ada"
        ));
        let failed = settings_view(Err("it threw".into()), &current, "fields/");
        assert!(matches!(
            Node::read(&failed["tree"]),
            Node::Status { text, .. } if text == "Its settings page failed: it threw"
        ));
    }
}
