//! The editors of plugins' tree inputs (ADR 0045). A draft is the
//! interface's state; the editor only edits it. Each edit goes back as
//! the draft, and a draft changed on another interface replaces what
//! the editor shows once this one's own edits have come back.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{App, AppContext as _, Entity};
use serde_json::Value;
use tau_ui_kit::input::{InputEvent, TextInput};
use tau_ui_plugin::{
    Handle,
    tree::{InputSpec, OnSubmit},
};

/// One input's editor, and what it knows of its draft.
struct Editor {
    input: Entity<TextInput>,
    /// What the editor showed last: an edit is a change from it.
    shown: String,
    /// The drafts it sent that the state has not come back with yet,
    /// oldest first.
    sent: Vec<String>,
    /// What its Enter does, as the tree drew it last.
    on_submit: Option<OnSubmit>,
    /// The focus number it last took the keys for.
    focused: Option<u64>,
}

/// Every input's editor in one window, by plugin and key.
#[derive(Default)]
pub struct TreeInputs {
    editors: HashMap<String, Editor>,
}

/// The editor of `spec`'s input for `plugin`, made the first time from
/// its draft.
pub fn editor(
    inputs: &Rc<RefCell<TreeInputs>>,
    handle: &Handle,
    spec: InputSpec,
    cx: &mut App,
) -> Entity<TextInput> {
    let id = format!("{}\u{0}{}", handle.plugin(), spec.key);
    let found = {
        let mut all = inputs.borrow_mut();
        all.editors.get_mut(&id).map(|editor| {
            editor.on_submit = spec.on_submit.clone();
            let refocus = spec.focus.is_some() && spec.focus != editor.focused;
            editor.focused = spec.focus;
            let replace =
                match editor.sent.iter().position(|sent| *sent == spec.draft) {
                    // Its own edits came back, up to this one.
                    Some(at) => {
                        editor.sent.drain(..=at);
                        None
                    }
                    // Changed elsewhere, once its own are all back; a
                    // secret is only ever what this editor holds.
                    None if !spec.secret
                        && editor.sent.is_empty()
                        && editor.shown != spec.draft =>
                    {
                        editor.shown = spec.draft.clone();
                        Some(spec.draft.clone())
                    }
                    None => None,
                };
            (editor.input.clone(), replace, refocus)
        })
    };
    let (input, refocus) = match found {
        Some((input, Some(draft), refocus)) => {
            input.update(cx, |input, cx| input.set_text(draft, cx));
            (input, refocus)
        }
        Some((input, None, refocus)) => (input, refocus),
        None => {
            let refocus = spec.focus.is_some();
            (make(inputs, handle, &id, spec, cx), refocus)
        }
    };
    if refocus {
        use gpui::Focusable as _;
        let focus = input.read(cx).focus_handle(cx);
        handle.focus(&focus, cx);
    }
    input
}

/// Makes the editor `id`, showing `spec`'s draft, that sends its edits
/// as the draft and carries out its Enter.
fn make(
    inputs: &Rc<RefCell<TreeInputs>>,
    handle: &Handle,
    id: &str,
    spec: InputSpec,
    cx: &mut App,
) -> Entity<TextInput> {
    let InputSpec {
        key,
        draft,
        placeholder,
        multiline,
        keep,
        secret,
        focus,
        on_submit,
    } = spec;
    let input = cx.new(|cx| {
        let mut input = TextInput::new(placeholder, cx);
        if multiline {
            input = input.multiline();
        }
        if keep {
            input = input.keep_on_submit();
        }
        if secret {
            input = input.masked();
        }
        input.set_text(draft.clone(), cx);
        input
    });
    {
        let (inputs, handle, id) =
            (inputs.clone(), handle.clone(), id.to_owned());
        cx.observe(&input, move |input, cx| {
            let text = input.read(cx).text().to_owned();
            {
                let mut all = inputs.borrow_mut();
                let Some(editor) = all.editors.get_mut(&id) else {
                    return;
                };
                if editor.shown == text {
                    return;
                }
                // A secret's draft stays in this editor.
                if secret {
                    editor.shown = text;
                    return;
                }
                editor.shown = text.clone();
                editor.sent.push(text.clone());
            }
            let value = (!text.is_empty()).then(|| Value::String(text));
            handle.set_ui(key.clone(), value, cx);
        })
        .detach();
    }
    {
        let (inputs, id) = (inputs.clone(), id.to_owned());
        cx.subscribe(&input, move |_, event: &InputEvent, cx| {
            let InputEvent::Submit(text) = event;
            let on_submit = inputs
                .borrow()
                .editors
                .get(&id)
                .and_then(|editor| editor.on_submit.clone());
            if let Some(on_submit) = on_submit {
                on_submit(text.clone(), cx);
            }
        })
        .detach();
    }
    inputs.borrow_mut().editors.insert(
        id.to_owned(),
        Editor {
            input: input.clone(),
            shown: draft,
            sent: Vec::new(),
            on_submit,
            focused: focus,
        },
    );
    input
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tau_ui_plugin::Request;

    use super::*;

    fn spec(draft: &str) -> InputSpec {
        InputSpec {
            key: "note.draft".into(),
            draft: draft.into(),
            placeholder: String::new(),
            multiline: false,
            keep: false,
            secret: false,
            focus: None,
            on_submit: None,
        }
    }

    /// A secret's editor sends nothing as its draft: what is typed in it
    /// stays in this window, and no draft from elsewhere replaces it.
    #[gpui::test]
    fn a_secret_stays_in_its_editor(cx: &mut gpui::TestAppContext) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let seen = sent.clone();
        let handle = Handle::new(
            "p",
            Rc::new(move |_, request, _| seen.borrow_mut().push(request)),
        );
        let inputs = Rc::new(RefCell::new(TreeInputs::default()));
        let secret = |draft: &str| InputSpec {
            secret: true,
            ..spec(draft)
        };
        let input = cx.update(|cx| editor(&inputs, &handle, secret(""), cx));
        input.update(cx, |input, cx| input.set_text("hunter2", cx));
        cx.run_until_parked();
        assert!(sent.borrow().is_empty(), "{:?}", sent.borrow());
        cx.update(|cx| editor(&inputs, &handle, secret("elsewhere"), cx));
        assert_eq!(
            input.read_with(cx, |input, _| input.text().to_owned()),
            "hunter2"
        );
    }

    /// An editor shows its draft, sends each edit back as the draft,
    /// keeps its own edits while the state has not come back with them,
    /// and shows a draft changed elsewhere once it has, without sending
    /// that change back.
    #[gpui::test]
    fn an_editor_follows_its_draft(cx: &mut gpui::TestAppContext) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let seen = sent.clone();
        let handle = Handle::new(
            "p",
            Rc::new(move |_, request, _| seen.borrow_mut().push(request)),
        );
        let inputs = Rc::new(RefCell::new(TreeInputs::default()));
        let text = |input: &Entity<TextInput>,
                    cx: &mut gpui::TestAppContext| {
            input.read_with(cx, |input, _| input.text().to_owned())
        };

        let input = cx.update(|cx| editor(&inputs, &handle, spec("hi"), cx));
        assert_eq!(text(&input, cx), "hi");
        input.update(cx, |input, cx| input.set_text("hi!", cx));
        cx.run_until_parked();
        assert_eq!(
            *sent.borrow(),
            [Request::Ui {
                key: "note.draft".into(),
                value: Some(json!("hi!")),
            }]
        );

        // Drawn before the state came back with the edit: it stays.
        cx.update(|cx| editor(&inputs, &handle, spec("hi"), cx));
        assert_eq!(text(&input, cx), "hi!");
        // The state comes back with it, then another interface changes it.
        cx.update(|cx| editor(&inputs, &handle, spec("hi!"), cx));
        cx.update(|cx| editor(&inputs, &handle, spec("elsewhere"), cx));
        assert_eq!(text(&input, cx), "elsewhere");
        cx.run_until_parked();
        assert_eq!(sent.borrow().len(), 1);

        // Emptied, it forgets the draft.
        input.update(cx, |input, cx| input.set_text("", cx));
        cx.run_until_parked();
        assert_eq!(
            sent.borrow().last(),
            Some(&Request::Ui {
                key: "note.draft".into(),
                value: None,
            })
        );
    }
}
