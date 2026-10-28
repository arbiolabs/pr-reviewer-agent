//! Settings → Review message: the template of the review body the reviewer posts, edited with its `{{variables}}`
//! coloured in place, a live preview filled with the daemon's samples, and Save or Reset through `settings.set`.

use std::ops::Range;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Editor, EditorState, InputEvent, TextDecoration, TextDecorationCollection};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::*;
use pr_review_tui::model::SaveState;
use pr_review_tui::protocol::{SummaryTemplate, Text};
use pr_review_tui::update::{Msg, write_block};

use crate::kit::{Tone, button};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::Shell;

/// The daemon's own limit (`MAX_TEMPLATE_CHARS`): longer is refused, so Save says so first.
const MAX_CHARS: usize = 4000;
const EDITOR_HEIGHT: Pixels = px(240.);
const VARIABLES_HEIGHT: Pixels = px(180.);

/// The editor's state, made the first time Settings draws, and the daemon's text it last loaded: a newer answer
/// replaces the text only while the reader has not changed it.
#[derive(Default)]
pub struct MessageEditor {
    pub editor: Option<Entity<EditorState>>,
    colors: Option<TextDecorationCollection>,
    loaded: Option<String>,
    /// Text a snapshot script put in before the editor existed.
    pub pending: Option<String>,
    _changes: Option<Subscription>,
}

/// Every `{{…}}` in `text`, as the daemon's `/\{\{([^{}]*)\}\}/g` finds them: the byte range and the name.
pub fn placeholders(text: &str) -> Vec<(Range<usize>, &str)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if &b[i..i + 2] != b"{{" {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        while j < b.len() && b[j] != b'{' && b[j] != b'}' {
            j += 1;
        }
        if b[j..].starts_with(b"}}") {
            out.push((i..j + 2, &text[i + 2..j]));
            i = j + 2;
        } else {
            i += 1;
        }
    }
    out
}

/// The names `text` uses that the daemon does not know, each once, in order.
pub fn unknown<'a>(text: &'a str, known: &[&str]) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for (_, name) in placeholders(text) {
        if !known.contains(&name) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The daemon's `fillTemplate`: empty variables go first, runs of blank lines collapse to one, then the rest are
/// filled in, so an empty variable alone on its line leaves no gap and a value is never rescanned.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let value = |name: &str| values.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
    let replace = |text: &str, with: &dyn Fn(&str) -> Option<String>| {
        let mut out = String::new();
        let mut at = 0;
        for (range, name) in placeholders(text) {
            out.push_str(&text[at..range.start]);
            out.push_str(&with(name).unwrap_or_else(|| text[range.clone()].to_string()));
            at = range.end;
        }
        out.push_str(&text[at..]);
        out
    };
    let shaped = replace(template, &|name| (value(name) == Some("")).then(String::new));
    let mut collapsed = String::with_capacity(shaped.len());
    let mut newlines = 0;
    for c in shaped.chars() {
        newlines = if c == '\n' { newlines + 1 } else { 0 };
        if newlines <= 2 {
            collapsed.push(c);
        }
    }
    replace(&collapsed, &|name| value(name).map(str::to_string))
}

impl Shell {
    /// Makes the editor the first time Settings draws, loads the daemon's text when it is new and the reader has not
    /// edited, and colours the placeholders: known in the accent, unknown in the danger hue.
    pub fn message_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = match &self.message.editor {
            Some(e) => e.clone(),
            None => {
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx).line_number(false).folding(false).searchable(false).soft_wrap(true)
                });
                let colors = editor.update(cx, |e, cx| e.create_decorations_collection(Vec::new(), cx));
                // The preview and the checks follow every keystroke.
                let changes = cx.subscribe(&editor, |_, _, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.notify();
                    }
                });
                self.message = MessageEditor {
                    editor: Some(editor.clone()),
                    colors: Some(colors),
                    pending: self.message.pending.take(),
                    _changes: Some(changes),
                    loaded: None,
                };
                editor
            }
        };
        let daemon = template(self).map(|t| t.value.to_string());
        let text = editor.read(cx).value().to_string();
        if let Some(value) = daemon.filter(|v| self.message.loaded.as_ref() != Some(v))
            && (self.message.loaded.is_none() || self.message.loaded.as_ref() == Some(&text))
        {
            editor.update(cx, |e, cx| e.set_value(value.clone(), window, cx));
            self.message.loaded = Some(value);
        }
        if let Some(staged) = self.message.pending.take() {
            editor.update(cx, |e, cx| e.set_value(staged, window, cx));
        }
        let t = crate::ui::colors(cx);
        let known = known(self);
        let text = editor.read(cx).value().to_string();
        let marks = placeholders(&text)
            .into_iter()
            .map(|(range, name)| {
                let hue = if known.contains(&name) { t.accent } else { t.danger };
                TextDecoration::new(
                    range,
                    HighlightStyle { color: Some(hue), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() },
                )
            })
            .collect();
        if let Some(c) = &self.message.colors {
            c.set(marks, cx);
        }
    }

    /// Puts `{{name}}` at the caret and gives the editor the keyboard back.
    fn insert_variable(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = &self.message.editor {
            e.update(cx, |e, cx| {
                e.insert(format!("{{{{{name}}}}}"), window, cx);
                e.focus(window, cx);
            });
        }
        cx.notify();
    }
}

/// What Save leaves once the daemon accepts the staged text: its answer names the text and the Saved line shows. A
/// snapshot stages it, since a playback refuses every command.
pub fn stage_saved(s: &mut Shell) {
    let Some(text) = s.message.pending.clone() else { return };
    if let Some(t) = s.model.agent.settings.as_mut().and_then(|x| x.summary_template.as_mut()) {
        t.value = Text::new(&text);
        t.is_default = false;
    }
    s.model.agent.template_save = SaveState::Saved(text);
}

fn template(s: &Shell) -> Option<&SummaryTemplate> {
    s.model.agent.settings.as_ref()?.summary_template.as_ref()
}

fn known(s: &Shell) -> Vec<&str> {
    template(s).map(|t| t.variables.iter().map(|v| v.name.as_str()).collect()).unwrap_or_default()
}

/// Why Save can't act now, or `None` when it can.
fn save_block(s: &Shell, t: &SummaryTemplate, text: &str, unknown: &[&str]) -> Option<String> {
    if let Some(why) = write_block(&s.model, "settings.set") {
        return Some(crate::kit::sentence(&format!("can't save here: {why}")));
    }
    if matches!(s.model.agent.template_save, SaveState::Sending { .. }) {
        return Some("Saving…".into());
    }
    if !unknown.is_empty() {
        return Some(String::new());
    }
    if text.chars().count() > MAX_CHARS {
        return Some(String::new());
    }
    let saved = if t.is_default { "" } else { t.value.as_str() };
    let typed = if text == t.default.as_str() { "" } else { text };
    (typed == saved).then(|| "Nothing changed since the last save.".into())
}

pub fn section(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let say = |words: String| div().p(space::S4).text_size(text::BODY).text_color(t.muted).child(words);
    let (Some(tpl), Some(editor)) = (template(s), s.message.editor.as_ref()) else {
        let words = match &s.model.agent.error {
            Some(why) => crate::kit::sentence(&format!("this reviewer can't change its review message: {why}")),
            None if s.model.agent.settings.is_some() => {
                "This reviewer is older than the review message setting. Update it to change the message.".into()
            }
            None if !s.model.link.accepts_events() => "Connect to the reviewer to change its review message.".into(),
            None => "Asking the reviewer for its review message…".into(),
        };
        return card(t, div().child(say(words)));
    };
    let text = editor.read(cx).value().to_string();
    let known: Vec<&str> = tpl.variables.iter().map(|v| v.name.as_str()).collect();
    let unknown = unknown(&text, &known);
    let length = text.chars().count();
    let samples: Vec<(&str, &str)> = tpl.variables.iter().map(|v| (v.name.as_str(), v.sample.as_str())).collect();
    let preview = fill(if text.is_empty() { tpl.default.as_str() } else { &text }, &samples);

    let check = if !unknown.is_empty() {
        let names = unknown.iter().map(|n| format!("{{{{{n}}}}}")).collect::<Vec<_>>().join(", ");
        let noun = if unknown.len() == 1 { "variable" } else { "variables" };
        Some((t.danger, format!("Unknown {noun} {names}. Pick one from the list, or remove it.")))
    } else if length > MAX_CHARS {
        Some((t.danger, format!("{length} characters; the reviewer takes at most {MAX_CHARS}.")))
    } else if text.is_empty() {
        Some((t.muted, "Empty: saving restores the default message.".to_string()))
    } else {
        None
    };
    let block = save_block(s, tpl, &text, &unknown);
    let status = match &s.model.agent.template_save {
        SaveState::Idle => None,
        SaveState::Sending { .. } => Some((t.muted, "Saving…".to_string())),
        SaveState::Saved(v) if v.is_empty() => {
            Some((t.ok, "Restored the default. Reviews prepared from now on use it.".into()))
        }
        SaveState::Saved(_) => Some((t.ok, "Saved. Reviews prepared from now on use this message.".into())),
        SaveState::Refused(why) => Some((t.danger, crate::kit::sentence(&format!("not saved: {why}")))),
        SaveState::TimedOut => Some((t.danger, "The reviewer did not answer in time. Try again.".into())),
    };
    let save = block.is_none().then(|| {
        let typed = if text == tpl.default.as_str() { String::new() } else { text.clone() };
        crate::ui::on(cx, move |s, cx| s.send(Msg::SetSummaryTemplate(typed.clone()), cx))
    });
    let may_reset = !tpl.is_default && write_block(&s.model, "settings.set").is_none();
    let reset = may_reset.then(|| {
        let default = tpl.default.to_string();
        crate::ui::on(cx, move |s, cx| {
            s.message.pending = Some(default.clone());
            s.message.loaded = Some(default.clone());
            s.send(Msg::SetSummaryTemplate(String::new()), cx)
        })
    });

    let mut variables = div().flex().flex_col().py(space::S1);
    for v in &tpl.variables {
        let name = v.name.to_string();
        variables = variables.child(
            div()
                .id(SharedString::from(format!("var-{name}")))
                .flex()
                .items_baseline()
                .gap(space::S3)
                .px(space::S4)
                .py(px(5.))
                .cursor_pointer()
                .hover(|h| h.bg(t.hover))
                .on_click(cx.listener(move |s, _: &ClickEvent, window, cx| s.insert_variable(&name, window, cx)))
                .child(
                    div()
                        .flex_none()
                        .w(px(150.))
                        .font_family(text::MONO)
                        .text_size(text::CODE)
                        .text_color(t.accent)
                        .child(format!("{{{{{}}}}}", v.name)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(text::SMALL)
                        .line_height(text::LINE_TIGHT)
                        .text_color(t.muted)
                        .child(v.description.to_string()),
                ),
        );
    }

    let label = |words: &'static str| {
        div().px(space::S4).pt(space::S3).pb(space::S2).text_size(text::SMALL).text_color(t.muted).child(words)
    };
    let body = div()
        .flex()
        .flex_col()
        .child(label("The summary the reviewer posts on each PR. Inline comments are not part of it."))
        .child(div().px(space::S4).child(Editor::new(editor).h(EDITOR_HEIGHT).small()))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(space::S3)
                .px(space::S4)
                .pt(space::S2)
                .text_size(text::SMALL)
                .child(match check {
                    Some((hue, words)) => div().text_color(hue).child(words),
                    None => div().text_color(t.ok).child("Every variable is known."),
                })
                .child(div().flex_none().text_color(t.faint).child(format!("{length} / {MAX_CHARS}"))),
        )
        .child(label("Variables. Click one to put it at the caret."))
        .child(
            div()
                .id("message-variables")
                .flex_none()
                .h(VARIABLES_HEIGHT)
                .border_t_1()
                .border_b_1()
                .border_color(t.border)
                .child(variables)
                .overflow_y_scrollbar(),
        )
        .child(label("Preview, with sample values"))
        .child(
            div().px(space::S4).child(
                div()
                    .p(space::S3)
                    .rounded(radius::SM)
                    .bg(t.code_bg)
                    .font_family(text::MONO)
                    .text_size(text::CODE)
                    .line_height(text::LINE_CODE)
                    .text_color(t.text)
                    .whitespace_normal()
                    .children(preview.trim_end().lines().map(|l| div().min_h(text::LINE_CODE).child(l.to_string()))),
            ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .mt(space::S4)
                .p(space::S4)
                .border_t_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .child(button(t, "message-save", "Save", None, Tone::Primary, save.flatten()))
                        .child(button(t, "message-reset", "Reset to default", None, Tone::Secondary, reset.flatten()))
                        .children(status.map(|(hue, words)| div().text_size(text::SMALL).text_color(hue).child(words)))
                        .children(
                            block
                                .filter(|why| !why.is_empty() && status_is_idle(s))
                                .map(|why| div().text_size(text::SMALL).text_color(t.muted).child(why)),
                        ),
                )
                .child(div().text_size(text::SMALL).line_height(text::LINE_BODY).text_color(t.faint).child(
                    "A review already waiting for approval keeps the message it was prepared with. The reviewer's \
                     hidden marker is always added after the message.",
                )),
        );
    card(t, body)
}

fn status_is_idle(s: &Shell) -> bool {
    matches!(s.model.agent.template_save, SaveState::Idle)
}

fn card(t: &Colors, content: Div) -> Div {
    crate::jev::titled(
        t,
        "Review message",
        div()
            .flex()
            .flex_col()
            .pb(space::S1)
            .rounded(radius::LG)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .overflow_hidden()
            .child(content),
    )
}

#[cfg(test)]
mod tests {
    use super::{fill, placeholders, unknown};
    use pr_review_tui::protocol::ServerMessage;

    fn protocol(rel: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol").join(rel)).unwrap()
    }

    #[test]
    fn placeholders_match_the_daemons_pattern() {
        let names = |s: &str| placeholders(s).into_iter().map(|(r, n)| (r, n.to_string())).collect::<Vec<_>>();
        assert_eq!(names("a {{repo}} b"), [(2..10, "repo".to_string())]);
        assert_eq!(names("{{{repo}}}"), [(1..9, "repo".to_string())]);
        assert_eq!(names("{{ x }}{{}}{{a{b}}"), [(0..7, " x ".to_string()), (7..11, String::new())]);
        assert_eq!(names("é{{repo}"), []);
        assert_eq!(unknown("{{repo}} {{Repo}} {{Repo}} {{nope}}", &["repo"]), ["Repo", "nope"]);
    }

    #[test]
    fn the_preview_matches_the_daemons_rendering_of_the_default_with_the_samples() {
        let Ok(ServerMessage::Settings(s)) = ServerMessage::parse(&protocol("fixtures/server.settings.json")) else {
            panic!("server.settings.json is a settings answer")
        };
        let t = s.summary_template.expect("the fixture carries the template");
        let samples: Vec<(&str, &str)> = t.variables.iter().map(|v| (v.name.as_str(), v.sample.as_str())).collect();
        assert_eq!(fill(t.default.as_str(), &samples), protocol("fixtures/summary-template.preview.md"));
    }

    #[test]
    fn an_empty_variable_alone_on_its_line_leaves_no_gap_and_a_value_is_never_rescanned() {
        let values = [("a", "{{b}}"), ("b", ""), ("c", "x\n\n\n\ny")];
        assert_eq!(fill("1\n\n{{b}}\n\n{{a}}\n\n{{c}}\n{{zz}}\n", &values), "1\n\n{{b}}\n\nx\n\n\n\ny\n{{zz}}\n");
    }
}
