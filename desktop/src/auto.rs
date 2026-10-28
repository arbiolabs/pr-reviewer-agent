//! Settings → Reviewer → Automatic reviews: whether the daemon starts reviews on its own for the PRs that request
//! the account's review, how often it asks GitHub, which repositories count, and what its last poll did.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::*;
use pr_review_tui::model::SaveState;
use pr_review_tui::protocol::{AutoReview, AutoReviewParams, RepoParams};
use pr_review_tui::update::{Msg, write_block};

use crate::kit::{Tone, button, icon, segmented};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::Shell;

const INTERVALS: [(u32, &str); 3] = [(5, "5 min"), (15, "15 min"), (30, "30 min")];

/// The two repository fields, made the first time Settings draws, and the lists they last loaded from the daemon:
/// a newer answer replaces a field only while the reader has not edited it.
#[derive(Default)]
pub struct RepoFields {
    pub include: Option<Entity<InputState>>,
    pub exclude: Option<Entity<InputState>>,
    loaded: Option<(String, String)>,
}

/// A field's text as the list the daemon takes: names split on commas and spaces. The first entry that is not
/// `owner/name` is the error.
pub fn parse(field: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = field.split([',', ' ', '\n']).filter(|s| !s.is_empty()).map(str::to_string).collect();
    let ok = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    match names.iter().find(|n| !matches!(n.split_once('/'), Some((o, r)) if ok(o) && ok(r))) {
        Some(bad) => Err(format!("“{bad}” is not owner/name.")),
        None => Ok(names),
    }
}

fn joined(names: &[pr_review_tui::protocol::Text]) -> String {
    names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
}

impl Shell {
    pub fn repo_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            input
        };
        if self.repos.include.is_none() {
            self.repos.include = Some(field("Every repository", window, cx));
            self.repos.exclude = Some(field("None", window, cx));
        }
        let (Some(include), Some(exclude)) = (self.repos.include.clone(), self.repos.exclude.clone()) else { return };
        let Some(a) = self.model.agent.settings.as_ref().and_then(|s| s.auto_review.as_ref()) else { return };
        let daemon = (joined(&a.repos.include), joined(&a.repos.exclude));
        let typed = (include.read(cx).value().to_string(), exclude.read(cx).value().to_string());
        if self.repos.loaded.as_ref() != Some(&daemon)
            && (self.repos.loaded.is_none() || self.repos.loaded.as_ref() == Some(&typed))
        {
            include.update(cx, |i, cx| i.set_value(daemon.0.clone(), window, cx));
            exclude.update(cx, |i, cx| i.set_value(daemon.1.clone(), window, cx));
            self.repos.loaded = Some(daemon);
        }
    }
}

fn save(cx: &Context<Shell>, params: AutoReviewParams) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
    cx.listener(move |s, _: &bool, _, cx| s.send(Msg::SetAutoReview(params.clone()), cx))
}

/// What publishing does with a review the poll starts, in the reader's words.
fn publishing(t: &Colors, mode: Option<&str>) -> Option<(Hsla, &'static str)> {
    match mode? {
        "approval" => Some((t.muted, "Reviews still wait for your approval before posting.")),
        "comment" | "silent_no_findings" => {
            Some((t.warn, "Reviews post to GitHub without asking: the publish mode isn't approval."))
        }
        "dry_run" => Some((t.muted, "Reviews are recorded and never posted: the publish mode is dry run.")),
        _ => None,
    }
}

/// The last poll as one line: when, and what it found and started, or why it failed.
pub fn last_poll(t: &Colors, a: &AutoReview) -> Option<(IconName, Hsla, String)> {
    let Some(p) = &a.last_poll else {
        return a.enabled.then(|| {
            (IconName::Clock, t.muted, "Not checked yet: the reviewer checks shortly after it starts.".to_string())
        });
    };
    let at = crate::kit::clock(p.at.as_str());
    Some(match &p.error {
        Some(why) => (
            IconName::CircleAlert,
            t.danger,
            crate::kit::sentence(&format!("last check at {at} failed: {why}; it tries again later")),
        ),
        None => (
            IconName::CircleCheck,
            t.ok,
            format!("Last checked at {at}: found {}, started {}, skipped {}.", p.found, p.started, p.skipped),
        ),
    })
}

pub fn section(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let say = |words: String| {
        div().p(space::S4).text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(words)
    };
    let Some(settings) = &m.agent.settings else {
        let words = match &m.agent.error {
            Some(why) => crate::kit::sentence(&format!("this reviewer can't review on its own: {why}")),
            None if !m.link.accepts_events() => "Connect to the reviewer to set up automatic reviews.".into(),
            None => "Asking the reviewer for its settings…".into(),
        };
        return card(t, say(words));
    };
    let Some(a) = &settings.auto_review else {
        return card(t, say("This reviewer is older than automatic reviews. Update it to turn them on.".into()));
    };
    let block = write_block(m, "settings.set");
    let busy = matches!(m.agent.auto_save, SaveState::Sending { .. });
    let can = block.is_none() && !busy;
    let line = |words: String, hue: Hsla| {
        div().text_size(text::SMALL).line_height(text::LINE_BODY).text_color(hue).child(words)
    };
    let label =
        |words: &'static str| div().w(px(180.)).flex_none().text_size(text::BODY).text_color(t.text).child(words);
    let row = || div().flex().items_center().gap(space::S3);

    let head = div()
        .flex()
        .items_start()
        .gap(space::S3)
        .child(
            Switch::new("auto-enabled")
                .checked(a.enabled)
                .disabled(!can)
                .color(t.accent)
                .accessibility_label("Automatic reviews")
                .on_click(save(cx, AutoReviewParams { enabled: Some(!a.enabled), ..Default::default() })),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(text::BODY)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Review PRs that request my review"),
                )
                .child(line(
                    format!(
                        "Every {} minutes the reviewer asks GitHub, as the account in use, for open PRs waiting on \
                         your review, and starts each commit it hasn't reviewed, at most 5 at a time. Archived \
                         PRs stay archived. Each review costs model usage.",
                        a.interval_minutes
                    ),
                    t.muted,
                )),
        );

    let every = segmented(
        t,
        INTERVALS.map(|(n, name)| {
            let params = AutoReviewParams { interval_minutes: Some(n), ..Default::default() };
            (
                format!("auto-every-{n}"),
                name,
                "",
                a.interval_minutes == n,
                format!("Check every {n} minutes"),
                crate::ui::click(cx, move |s, cx| {
                    if !busy && write_block(&s.model, "settings.set").is_none() {
                        s.send(Msg::SetAutoReview(params.clone()), cx)
                    }
                }),
                None,
            )
        }),
    );
    let teams = Switch::new("auto-teams")
        .checked(a.include_team_requests)
        .disabled(!can)
        .color(t.accent)
        .small()
        .label("Include requests to my teams")
        .on_click(save(
            cx,
            AutoReviewParams { include_team_requests: Some(!a.include_team_requests), ..Default::default() },
        ));

    let typed = |f: &Option<Entity<InputState>>| f.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
    let (include, exclude) = (parse(&typed(&s.repos.include)), parse(&typed(&s.repos.exclude)));
    let changed =
        typed(&s.repos.include) != joined(&a.repos.include) || typed(&s.repos.exclude) != joined(&a.repos.exclude);
    let repos_save = match (&include, &exclude) {
        (Ok(i), Ok(e)) if can && changed => {
            let params = AutoReviewParams {
                repos: Some(RepoParams { include: Some(i.clone()), exclude: Some(e.clone()) }),
                ..Default::default()
            };
            crate::ui::on(cx, move |s, cx| s.send(Msg::SetAutoReview(params.clone()), cx))
        }
        _ => None,
    };
    let field = |f: &Option<Entity<InputState>>| {
        div()
            .flex_1()
            .min_w_0()
            .font_family(text::MONO)
            .text_size(text::CODE)
            .children(f.as_ref().map(|i| Input::new(i).disabled(block.is_some())))
    };
    let problem = include.as_ref().err().or(exclude.as_ref().err()).cloned();

    let status = match &m.agent.auto_save {
        SaveState::Sending { .. } => Some((t.muted, "Saving…".to_string())),
        SaveState::Refused(why) => Some((t.danger, crate::kit::sentence(&format!("not saved: {why}")))),
        SaveState::TimedOut => Some((t.danger, "The reviewer did not answer in time. Try again.".to_string())),
        SaveState::Idle | SaveState::Saved(_) => {
            block.map(|why| (t.muted, crate::kit::sentence(&format!("can't change it here: {why}"))))
        }
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(space::S4)
        .p(space::S4)
        .child(head)
        .children(publishing(t, settings.publish_mode.as_ref().map(|p| p.as_str())).map(|(hue, w)| line(w.into(), hue)))
        .child(row().child(label("Check every")).child(every))
        .child(row().child(label("Team requests")).child(teams))
        .child(row().child(label("Only these repositories")).child(field(&s.repos.include)))
        .child(row().child(label("Never these repositories")).child(field(&s.repos.exclude)))
        .child(
            row()
                .child(div().w(px(180.)).flex_none())
                .child(button(t, "auto-repos-save", "Save repositories", None, Tone::Secondary, repos_save))
                .child(div().flex_1().min_w_0().child(match problem {
                    Some(why) => line(why, t.danger),
                    None => {
                        line("owner/name, separated by commas. A repository in both lists is skipped.".into(), t.faint)
                    }
                })),
        )
        .children(status.map(|(hue, w)| line(w, hue)))
        .children(last_poll(t, a).map(|(glyph, hue, words)| {
            div()
                .flex()
                .items_start()
                .gap(space::S2)
                .pt(space::S3)
                .border_t_1()
                .border_color(t.border)
                .child(div().pt(px(3.)).child(icon(glyph, px(14.), hue)))
                .child(line(words, if hue == t.danger { t.danger } else { t.text }).flex_1().min_w_0())
        }));
    card(t, body)
}

fn card(t: &Colors, content: Div) -> Div {
    crate::jev::titled(
        t,
        "Automatic reviews",
        div().flex().flex_col().rounded(radius::LG).border_1().border_color(t.border).bg(t.surface).child(content),
    )
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn repositories_are_owner_name_split_on_commas_and_spaces() {
        assert_eq!(parse(" acme/shop,acme/api  x.y/z-1 ").unwrap(), ["acme/shop", "acme/api", "x.y/z-1"]);
        assert_eq!(parse("").unwrap(), Vec::<String>::new());
        assert!(parse("acme").is_err());
        assert!(parse("acme/shop/extra").is_err());
        assert!(parse("acme/ü").is_err());
    }
}
