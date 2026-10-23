//! Settings → Agent: the model the daemon's built-in engine reviews with, picked from what it can run.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::model::SaveState;
use pr_review_tui::protocol::{ModelOption, Settings};
use pr_review_tui::update::Msg;

use crate::kit::{Tone, button, chip, icon};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::Shell;

/// The picker's own state: its search field and the row the reader chose, until Save.
#[derive(Default)]
pub struct Picker {
    pub search: Option<Entity<InputState>>,
    pub choice: Option<String>,
}

impl Shell {
    /// The search field lives in the window, so it is made the first time Settings draws.
    pub fn model_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.search.is_none() {
            self.picker.search = Some(cx.new(|cx| InputState::new(window, cx).placeholder("Search models")));
        }
    }
}

/// The models whose name or ref holds `query`, by provider in the daemon's order (preferred first).
pub fn groups<'a>(models: &'a [ModelOption], query: &str) -> Vec<(&'a str, Vec<&'a ModelOption>)> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<(&str, Vec<&ModelOption>)> = Vec::new();
    let hits = models.iter().filter(|m| {
        q.is_empty() || m.name.as_str().to_lowercase().contains(&q) || m.reference.as_str().to_lowercase().contains(&q)
    });
    for m in hits {
        match out.iter_mut().find(|(p, _)| *p == m.provider.as_str()) {
            Some((_, rows)) => rows.push(m),
            None => out.push((m.provider.as_str(), vec![m])),
        }
    }
    out
}

/// A context window in tokens as a reader says it: 200K, 1M.
pub fn context(tokens: u64) -> String {
    match tokens {
        n if n >= 1_000_000 => format!("{}M", (n as f64 / 100_000.).round() / 10.),
        n if n >= 1_000 => format!("{}K", n / 1_000),
        n => n.to_string(),
    }
}

/// Where the model in use was chosen, and whether Settings may change it.
fn source(s: &Settings) -> (String, bool) {
    let path = s.config_path.as_str();
    match s.model.source.as_str() {
        "config" => (format!("Saved in {path}."), true),
        "default" => ("The first model available: nothing has chosen one yet.".into(), true),
        "PR_REVIEW_MODEL" => (
            "Set by PR_REVIEW_MODEL in the reviewer's environment. Remove it and restart the reviewer to choose here."
                .into(),
            false,
        ),
        other => (format!("Set by the reviewer's {other}. Remove it and restart the reviewer to choose here."), false),
    }
}

pub fn section(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let say = |words: String| {
        div().p(space::S4).text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(words)
    };
    let Some(settings) = &m.agent.settings else {
        let words = match &m.agent.error {
            Some(why) => crate::kit::sentence(&format!("this reviewer can't change its model: {why}")),
            None if !m.link.accepts_events() => "Connect to the reviewer to choose its model.".into(),
            None => "Asking the reviewer for its models…".into(),
        };
        return card(t, div().child(engine_row(t, None)).child(say(words)));
    };
    let (from, may_change) = source(settings);
    let current = settings.model.reference.as_str();
    let choice = s.picker.choice.as_deref().unwrap_or(current);
    let query = s.picker.search.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
    // The page scrolls the whole list, and the search narrows it: a second scroll area inside the page shrank its rows
    // to its height instead of scrolling them.
    let mut list = div().flex().flex_col();
    let found = groups(&settings.models, &query);
    if found.is_empty() {
        list = list.child(say(format!("No model matches “{}”.", query.trim())));
    }
    for (provider, rows) in found {
        list = list.child(
            div()
                .px(space::S4)
                .pt(space::S3)
                .pb(space::S1)
                .text_size(text::CAPTION)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.faint)
                .child(provider.to_uppercase()),
        );
        for o in rows {
            list = list.child(model_row(
                t,
                o,
                o.reference.as_str() == current,
                o.reference.as_str() == choice,
                may_change,
                cx,
            ));
        }
    }
    let saving = matches!(m.agent.save, SaveState::Sending { .. });
    let save = may_change && choice != current && !saving && m.may_write("settings.set");
    let pick = choice.to_string();
    let status = match &m.agent.save {
        SaveState::Idle => None,
        SaveState::Sending { model, .. } => Some((t.muted, format!("Saving {model}…"))),
        SaveState::Saved(model) => Some((t.ok, format!("Saved. Reviews started from now on use {model}."))),
        SaveState::Refused(why) => Some((t.danger, crate::kit::sentence(&format!("not saved: {why}")))),
        SaveState::TimedOut => Some((t.danger, "The reviewer did not answer in time. Try again.".into())),
    };
    let body = div()
        .child(engine_row(t, settings.models.iter().find(|o| o.reference.as_str() == current).map(|o| (o, current))))
        .child(div().px(space::S4).pb(space::S3).text_size(text::SMALL).text_color(t.muted).child(from))
        .children(s.picker.search.as_ref().map(|i| {
            div()
                .px(space::S4)
                .pb(space::S2)
                .border_t_1()
                .border_color(t.border)
                .pt(space::S3)
                .child(Input::new(i).cleanable(true))
        }))
        .child(list)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .p(space::S4)
                .border_t_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .child(button(
                            t,
                            "model-save",
                            "Save",
                            None,
                            Tone::Primary,
                            save.then(|| crate::ui::on(cx, move |s, cx| s.send(Msg::SetModel(pick.clone()), cx)))
                                .flatten(),
                        ))
                        .children(status.map(|(hue, words)| div().text_size(text::SMALL).text_color(hue).child(words))),
                )
                .child(div().text_size(text::SMALL).line_height(text::LINE_BODY).text_color(t.faint).child(
                    "The model is part of the reviewer's identity. A PR reviewed again with another model gets a new \
                         revision; the stored result of the old one is not reused.",
                )),
        );
    card(t, body)
}

/// The harness that runs reviews, and the model it runs them with now.
fn engine_row(t: &Colors, current: Option<(&ModelOption, &str)>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(space::S3)
        .px(space::S4)
        .pt(space::S4)
        .pb(space::S2)
        .child(icon(IconName::Cpu, px(15.), t.accent))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(match current {
                    Some((o, _)) => format!("Reviews with {}", o.name),
                    None => "Built-in review engine".to_string(),
                }))
                .children(current.map(|(_, r)| {
                    div().font_family(text::MONO).text_size(px(11.)).text_color(t.muted).child(r.to_string())
                })),
        )
}

fn model_row(
    t: &Colors,
    o: &ModelOption,
    current: bool,
    chosen: bool,
    may_change: bool,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let r = o.reference.to_string();
    let dot = div()
        .size(px(14.))
        .flex_none()
        .rounded(radius::PILL)
        .border_1()
        .border_color(if chosen { t.accent } else { t.border_strong })
        .flex()
        .items_center()
        .justify_center()
        .when(chosen, |d| d.child(div().size(px(6.)).rounded(radius::PILL).bg(t.accent)));
    div()
        .id(SharedString::from(format!("model-{r}")))
        .flex()
        .items_center()
        .gap(space::S3)
        .px(space::S4)
        .py(px(7.))
        .when(chosen, |d| d.bg(t.selected))
        .when(may_change, |d| {
            d.cursor_pointer().hover(|h| h.bg(t.hover)).on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                s.picker.choice = Some(r.clone());
                cx.notify();
            }))
        })
        .child(dot)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().text_size(text::BODY).text_color(t.text).child(o.name.to_string()))
                .child(
                    div().font_family(text::MONO).text_size(px(11.)).text_color(t.muted).child(o.reference.to_string()),
                ),
        )
        .when(current, |d| d.child(chip(t, "In use", t.accent)))
        .when(o.preferred && !current, |d| d.child(chip(t, "Preferred", t.muted)))
        .child(div().w(px(48.)).text_size(text::SMALL).text_color(t.muted).child(context(o.context_window)))
        .child(chip(t, o.auth_source.to_string(), t.muted))
}

fn card(t: &Colors, content: Div) -> Div {
    crate::jev::titled(
        t,
        "Agent",
        div()
            .flex()
            .flex_col()
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
    use super::{ModelOption, context, groups};
    use pr_review_tui::protocol::Text;

    fn option(r: &str, name: &str) -> ModelOption {
        ModelOption {
            reference: Text::new(r),
            provider: Text::new(r.split('/').next().unwrap()),
            name: Text::new(name),
            context_window: 200_000,
            auth_source: Text::new("oauth"),
            preferred: false,
        }
    }

    #[test]
    fn search_keeps_the_providers_in_order_and_matches_name_or_ref() {
        let models =
            [option("anthropic/opus", "Opus"), option("openai/gpt", "GPT"), option("anthropic/sonnet", "Sonnet")];
        let names = |q: &str| {
            groups(&models, q)
                .into_iter()
                .map(|(p, rows)| (p, rows.iter().map(|m| m.name.to_string()).collect::<Vec<_>>()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(""),
            [("anthropic", vec!["Opus".to_string(), "Sonnet".into()]), ("openai", vec!["GPT".into()])]
        );
        assert_eq!(names(" SON "), [("anthropic", vec!["Sonnet".to_string()])]);
        assert_eq!(names("openai/"), [("openai", vec!["GPT".to_string()])]);
        assert!(names("nothing").is_empty());
    }

    #[test]
    fn context_windows_read_as_people_say_them() {
        assert_eq!(
            [context(200_000), context(1_000_000), context(1_048_576), context(1_500_000), context(512)],
            ["200K", "1M", "1M", "1.5M", "512"]
        );
    }
}
