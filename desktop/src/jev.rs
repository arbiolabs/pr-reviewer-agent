//! Settings → Risk check (Jev): the TypeSafe key, saved to the login keychain where the daemon reads it.
//!
//! Every Keychain call goes through `/usr/bin/security`, which then owns the item: its access list trusts
//! `/usr/bin/security`, the program the daemon reads it with (src/jev/key.ts), so the LaunchAgent's daemon reads it
//! without a prompt. Written through the Security framework, the item would trust only this app's own code signature,
//! and the daemon's read would prompt or fail. The key goes to `security -i` on stdin as one command line, never in
//! argv, so `ps` never shows it; nothing here keeps it after Save, and the page only ever says set or not set.

use std::io::Write as _;
use std::process::{Command, Stdio};

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::kit::{Tone, button, icon};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::{Shell, on};

const SECURITY: &str = "/usr/bin/security";

/// The item the daemon reads; PR_REVIEW_KEYCHAIN_SERVICE renames the service so a test never touches the real one.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub service: String,
    pub account: &'static str,
}

impl Item {
    pub fn from_env() -> Item {
        let service = std::env::var("PR_REVIEW_KEYCHAIN_SERVICE").ok().filter(|s| !s.is_empty());
        Item {
            service: service.unwrap_or_else(|| "io.github.diego-arbio-dev.pr-review".into()),
            account: "TYPESAFE_API_KEY",
        }
    }
}

/// What the page knows about the item: never its value.
#[derive(Clone, Debug, PartialEq)]
pub enum Stored {
    Set,
    NotSet,
    /// The Keychain did not answer; the words say why.
    Unknown(String),
}

/// The key field, its item, and what the last Save or Remove did.
pub struct KeyField {
    pub item: Item,
    pub stored: Stored,
    pub input: Option<Entity<InputState>>,
    /// The last Save's or Remove's outcome, for the line under the buttons: ok and its words.
    pub outcome: Option<(bool, String)>,
}

impl KeyField {
    pub fn new() -> KeyField {
        let item = Item::from_env();
        let stored = if cfg!(target_os = "macos") { lookup(&item) } else { Stored::NotSet };
        KeyField { item, stored, input: None, outcome: None }
    }
}

/// Save's label and whether Remove can act, for what the Keychain holds. When it can't tell, Remove may try.
pub fn controls(stored: &Stored) -> (&'static str, bool) {
    match stored {
        Stored::Set => ("Replace", true),
        Stored::NotSet => ("Save", false),
        Stored::Unknown(_) => ("Save", true),
    }
}

/// A key `security -i` reads back exactly as typed: one token, no quotes, backslashes or controls. Surrounding
/// whitespace from a paste is dropped.
pub fn validate(key: &str) -> Result<&str, &'static str> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Paste the key first.");
    }
    if key.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\' || !c.is_ascii()) {
        return Err(
            "That doesn't look like a TypeSafe key: it has spaces, quotes or other characters a key never has.",
        );
    }
    Ok(key)
}

/// The one line `security -i` reads from stdin: add the item, or update it in place (`-U`), with the key as its
/// password. The service is quoted the same way, since a test may rename it.
pub fn add_line(item: &Item, key: &str) -> String {
    format!("add-generic-password -U -s \"{}\" -a \"{}\" -w \"{key}\"\n", item.service, item.account)
}

fn run(args: &[&str], stdin: Option<&str>) -> Result<i32, String> {
    let mut child = Command::new(SECURITY)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't run {SECURITY}: {e}"))?;
    if let (Some(line), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(line.as_bytes()).map_err(|e| format!("couldn't talk to {SECURITY}: {e}"))?;
    }
    let status = child.wait().map_err(|e| format!("{SECURITY} did not finish: {e}"))?;
    Ok(status.code().unwrap_or(-1))
}

/// Exit 44 is the Keychain's "no such item".
const NOT_FOUND: i32 = 44;

/// Whether the item exists, from its attributes alone: the secret is not read, so this never prompts.
pub fn lookup(item: &Item) -> Stored {
    match run(&["find-generic-password", "-s", &item.service, "-a", item.account], None) {
        Ok(0) => Stored::Set,
        Ok(NOT_FOUND) => Stored::NotSet,
        Ok(code) => Stored::Unknown(format!("the Keychain answered {code}")),
        Err(e) => Stored::Unknown(e),
    }
}

pub fn save(item: &Item, key: &str) -> Result<(), String> {
    match run(&["-i"], Some(&add_line(item, key)))? {
        0 => Ok(()),
        code => Err(format!("the Keychain refused the key ({code})")),
    }
}

pub fn remove(item: &Item) -> Result<(), String> {
    match run(&["delete-generic-password", "-s", &item.service, "-a", item.account], None)? {
        0 | NOT_FOUND => Ok(()),
        code => Err(format!("the Keychain would not remove the key ({code})")),
    }
}

impl Shell {
    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.jev.input.clone() else { return };
        let typed = input.read(cx).value();
        self.jev.outcome = Some(match validate(&typed).map_err(str::to_string).and_then(|k| save(&self.jev.item, k)) {
            Ok(()) => {
                input.update(cx, |i, cx| i.set_value("", window, cx));
                (true, "Saved. The next review uses it; the reviewer needs no restart.".into())
            }
            Err(e) => (false, crate::kit::sentence(&e)),
        });
        self.jev.stored = lookup(&self.jev.item);
        cx.notify();
    }

    fn remove_key(&mut self, cx: &mut Context<Self>) {
        self.jev.outcome = Some(match remove(&self.jev.item) {
            Ok(()) => (true, "Removed. The next review runs without TypeSafe.".into()),
            Err(e) => (false, crate::kit::sentence(&e)),
        });
        self.jev.stored = lookup(&self.jev.item);
        cx.notify();
    }

    /// The field's state lives in the window, so it is made the first time Settings draws.
    pub fn key_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.jev.input.is_none() && cfg!(target_os = "macos") {
            self.jev.input =
                Some(cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Paste your TypeSafe key")));
        }
    }

    /// Whether a Settings field has the keyboard, so the page's single-key shortcuts stay out of it.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        [self.jev.input.as_ref(), self.picker.search.as_ref()]
            .into_iter()
            .flatten()
            .any(|i| i.read(cx).focus_handle(cx).is_focused(window))
            || self.message.editor.as_ref().is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window))
    }
}

/// Risk check (Jev): whether a key is saved, the masked field, Save and Remove. Elsewhere than macOS, only the note
/// that the environment is the way.
pub fn section(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let body = div().flex().flex_col().gap(space::S3).p(space::S4);
    let about = div().text_size(text::SMALL).line_height(text::LINE_BODY).text_color(t.muted).child(
        "Jev rates each change's risk so risky PRs get a deeper review. With a TypeSafe key it covers private \
         repositories too.",
    );
    let Some(input) = s.jev.input.as_ref().filter(|_| cfg!(target_os = "macos")) else {
        return card(
            t,
            body.child(about)
                .child(note(t, "Set TYPESAFE_API_KEY in the daemon's environment: the Keychain is macOS only.")),
        );
    };
    let (glyph, hue, word) = match &s.jev.stored {
        Stored::Set => (IconName::CircleCheck, t.ok, "A key is saved in your Keychain".to_string()),
        Stored::NotSet => (IconName::CircleX, t.muted, "No key saved".to_string()),
        Stored::Unknown(why) => (IconName::CircleAlert, t.warn, crate::kit::sentence(&format!("can't tell: {why}"))),
    };
    let (save_label, removable) = controls(&s.jev.stored);
    let field =
        div().flex_1().min_w_0().font_family(text::MONO).text_size(text::CODE).child(Input::new(input).cleanable(true));
    let body = body
        .child(about)
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(icon(glyph, px(15.), hue))
                .child(div().text_size(text::BODY).font_weight(FontWeight::SEMIBOLD).child(word)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(field)
                .child(button(
                    t,
                    "jev-save",
                    save_label,
                    None,
                    Tone::Primary,
                    (!input.read(cx).value().trim().is_empty()).then(|| Box::new(cx.listener(|s, _: &ClickEvent, window, cx| s.save_key(window, cx))) as crate::kit::OnClick),
                ))
                .child(button(t, "jev-remove", "Remove", None, Tone::Danger, if removable { on(cx, Shell::remove_key) } else { None })),
        )
        .children(s.jev.outcome.as_ref().map(|(ok, words)| {
            div().text_size(text::SMALL).text_color(if *ok { t.ok } else { t.danger }).child(words.clone())
        }))
        .child(note(
            t,
            "Saved as a generic password in your login keychain. TYPESAFE_API_KEY in the daemon's environment wins over \
             it.",
        ));
    card(t, body)
}

fn note(t: &Colors, words: &str) -> Div {
    div().text_size(text::SMALL).line_height(text::LINE_BODY).text_color(t.faint).child(words.to_string())
}

/// A Settings section: its title over a bordered card.
pub fn titled(t: &Colors, title: &str, content: Div) -> Div {
    div().flex().flex_col().gap(space::S3).child(crate::kit::overline(t, title)).child(content)
}

fn card(t: &Colors, content: Div) -> Div {
    titled(
        t,
        "Risk check (Jev)",
        div().flex().flex_col().rounded(radius::LG).border_1().border_color(t.border).bg(t.surface).child(content),
    )
}

#[cfg(test)]
mod tests {
    use super::{Command, Item, SECURITY, Stdio, Stored, add_line, controls, lookup, remove, save, validate};

    #[test]
    fn a_pasted_key_is_trimmed_and_anything_security_would_misread_is_refused() {
        assert_eq!(validate("  tsk_live_AbC-123.x/y+z=  \n"), Ok("tsk_live_AbC-123.x/y+z="));
        for bad in ["", "   ", "two words", "quo\"te", "back\\slash", "tab\there", "ключ"] {
            assert!(validate(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_buttons_follow_what_the_keychain_holds() {
        assert_eq!(controls(&Stored::NotSet), ("Save", false));
        assert_eq!(controls(&Stored::Set), ("Replace", true));
        assert_eq!(controls(&Stored::Unknown("locked".into())), ("Save", true));
    }

    #[test]
    fn the_key_goes_on_stdin_and_never_in_argv() {
        let item = Item { service: "prsa.test".into(), account: "TYPESAFE_API_KEY" };
        assert_eq!(
            add_line(&item, "test-not-a-real-key"),
            "add-generic-password -U -s \"prsa.test\" -a \"TYPESAFE_API_KEY\" -w \"test-not-a-real-key\"\n"
        );
    }

    /// The real round trip, under a test service only: `cargo test -- --ignored keychain` on macOS.
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn keychain_round_trip_under_a_test_service() {
        let item = Item {
            service: format!("io.github.diego-arbio-dev.pr-review.test-{}", std::process::id()),
            account: "TYPESAFE_API_KEY",
        };
        let read = || {
            let out = Command::new(SECURITY)
                .args(["find-generic-password", "-s", &item.service, "-a", item.account, "-w"])
                .stdin(Stdio::null())
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim_end().to_string()
        };
        assert_eq!(lookup(&item), Stored::NotSet);
        save(&item, "test-not-a-real-key").unwrap();
        assert_eq!((lookup(&item), read()), (Stored::Set, "test-not-a-real-key".to_string()));
        save(&item, "test-not-a-real-key-2").unwrap();
        assert_eq!(read(), "test-not-a-real-key-2");
        remove(&item).unwrap();
        assert_eq!(lookup(&item), Stored::NotSet, "one Remove clears it: the second Save updated, not duplicated");
        remove(&item).unwrap();
    }
}
