//! What floats over the window: the command palette, the sheets (a post, a cancel, a refusal), the shortcut
//! sheet, the jump-to input and the reducer's notices.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{AddState, ConfirmState, InputPurpose, Modal, Model, PrRef, Preview, Screen, fuzzy};
use pr_review_tui::protocol::{Command, ErrorCode, PrId};
use pr_review_tui::update::{again_block, can_confirm, can_mutate, write_block};
use std::ops::Range;

use crate::detail::who;
use crate::kit::{Tone, button, icon, keycap, overline, tip};
use crate::queue::{self, Card, Stage};
use crate::reviewer::{Ask, Reviewer};
use crate::setup::Tab as SettingsTab;
use crate::tokens::{Choice, Colors, layout, radius, space, text};
use crate::ui::{List, Overlay, Shell, on};

#[derive(Clone)]
enum Run {
    Card(usize),
    Pr(PrId),
    Direct(Mutation),
    Act(Action),
    GitHub,
    Archive,
    Tab(List),
    Usage,
    Keys,
    AddPr,
    Reviewer(Ask),
    Theme(Choice),
    Settings(SettingsTab),
}

struct Entry {
    group: &'static str,
    title: String,
    key: Option<&'static str>,
    /// Why it would do nothing now. Shown, never hidden, so the palette also teaches what exists.
    blocked: Option<String>,
    run: Run,
}

/// The reason every command shares on a read-only daemon, on each row it dims.
const READ_ONLY: &str = "read-only daemon";

fn mutation(m: &Model, mu: Mutation, title: String, key: &'static str, otherwise: &str) -> Entry {
    // On a read-only daemon every command is off for that one reason, whatever else holds.
    let blocked = if queue::read_only(m) {
        Some(READ_ONLY.into())
    } else {
        (!can_mutate(m, mu)).then(|| match write_block(m, mu.capability()) {
            Some(why) => why.to_string(),
            None if m.selected_pr().is_none() => "no PR is selected".into(),
            None => otherwise.to_string(),
        })
    };
    let run =
        if matches!(mu, Mutation::Start | Mutation::Retry) { Run::Direct(mu) } else { Run::Act(Action::Mutation(mu)) };
    Entry { group: "Review", title, key: Some(key), blocked, run }
}

/// Every command, contextual ones first: each card's decision, the selected PR's actions with the reason any
/// can't run, then going to a PR, then the views.
fn entries(m: &Model, r: &Reviewer, theme: Choice) -> Vec<Entry> {
    let mut out = Vec::new();
    for (i, c) in queue::cards(m).iter().enumerate() {
        let title = match c {
            Card::Setup { .. } => "Open Settings: reviews can't run yet".to_string(),
            Card::Approve { pr, .. } => format!("Review and post #{}", pr.number),
            Card::Posting { pr, .. } => format!("Check the posting on #{}", pr.number),
            Card::Findings { pr, findings } => {
                format!("Judge {} in #{}", queue::count(findings.len(), "finding"), pr.number)
            }
            Card::Failed { pr, run } if queue::ending(run) == Some(queue::Ending::Cancelled) => {
                format!("Review the cancelled #{} again", pr.number)
            }
            Card::Failed { pr, .. } => format!("Retry the failed review of #{}", pr.number),
            Card::Start { pr } => format!("Start a review of #{}", pr.number),
        };
        // A card whose action is a command is refused on a read-only daemon, like the card's own button.
        let command = matches!(c, Card::Failed { .. } | Card::Start { .. });
        let blocked = (command && queue::read_only(m)).then(|| READ_ONLY.to_string());
        out.push(Entry { group: "Needs you", title, key: None, blocked, run: Run::Card(i) });
    }
    for (p, _) in queue::running(m) {
        out.push(Entry {
            group: "Needs you",
            title: format!("Watch #{} live", p.number),
            key: None,
            blocked: None,
            run: Run::Pr(p.id.clone()),
        });
    }
    let n = m.selected_pr().map_or(String::new(), |p| format!(" #{}", p.number));
    out.push(mutation(
        m,
        Mutation::Publish,
        format!("Review and post the proof of{n}"),
        "P",
        "nothing is waiting for approval",
    ));
    out.push(mutation(
        m,
        Mutation::Start,
        format!("Start a review of{n} at its head"),
        "S",
        "a review already holds this head",
    ));
    out.push(mutation(
        m,
        Mutation::Again,
        format!("Review{n} again from scratch"),
        "⇧S",
        &again_block(m).unwrap_or_default(),
    ));
    out.push(mutation(
        m,
        Mutation::Retry,
        format!("Retry the failed review of{n}"),
        "⇧R",
        "its latest run has not failed",
    ));
    out.push(mutation(m, Mutation::Cancel, format!("Cancel the review of{n}"), "C", "no review is running on it"));
    out.push(Entry {
        group: "Review",
        title: "Review a PR by its URL…".into(),
        key: Some("⌘N"),
        blocked: add_block(m, r),
        run: Run::AddPr,
    });
    let archived = m.selected_pr().is_some_and(|p| p.archived());
    out.push(Entry {
        group: "Review",
        title: format!("{}{n}", if archived { "Unarchive" } else { "Archive" }),
        key: Some("E"),
        blocked: match (m.selected_pr(), write_block(m, "pr.archive")) {
            (None, _) => Some("no PR is selected".into()),
            (_, why) => why.map(String::from),
        },
        run: Run::Archive,
    });
    out.push(Entry {
        group: "Review",
        title: format!("Open{n} on GitHub"),
        key: Some("⌘⇧O"),
        blocked: m.selected_pr().is_none().then(|| "no PR is selected".to_string()),
        run: Run::GitHub,
    });
    for p in m.inbox() {
        out.push(Entry {
            group: "Go to",
            title: format!("#{} {} · {}", p.number, p.title, p.repository),
            key: None,
            blocked: None,
            run: Run::Pr(p.id.clone()),
        });
    }
    let selected = m.selected_pr().is_some();
    out.extend([
        Entry {
            group: "View", title: "Needs you".into(), key: Some("⌘1"), blocked: None, run: Run::Tab(List::Queue)
        },
        Entry { group: "View", title: "Done".into(), key: Some("⌘2"), blocked: None, run: Run::Tab(List::Done) },
        Entry {
            group: "View",
            title: "Archived".into(),
            key: Some("⌘3"),
            blocked: None,
            run: Run::Tab(List::Archived),
        },
        Entry { group: "View", title: "Usage".into(), key: Some("⌘4"), blocked: None, run: Run::Usage },
        Entry {
            group: "View",
            title: "Diff of the selected PR".into(),
            key: Some("D"),
            blocked: (!selected).then(|| "no PR is selected".to_string()),
            run: Run::Act(Action::OpenDiff),
        },
        Entry {
            group: "View",
            title: "Timeline of the selected PR".into(),
            key: Some("T"),
            blocked: (!selected).then(|| "no PR is selected".to_string()),
            run: Run::Act(Action::OpenTimeline),
        },
        Entry {
            group: "View",
            title: "Settings".into(),
            key: Some("⌘,"),
            blocked: None,
            run: Run::Act(Action::Goto(Screen::Settings)),
        },
        Entry {
            group: "App",
            title: "Refresh from the daemon".into(),
            key: Some("⌘R"),
            blocked: None,
            run: Run::Act(Action::Refresh),
        },
        Entry { group: "App", title: "Keyboard shortcuts".into(), key: Some("?"), blocked: None, run: Run::Keys },
    ]);
    out.extend(SettingsTab::ALL.map(|tab| {
        Entry {
            group: "Settings",
            title: match tab {
                SettingsTab::Review => "Settings: model, risk check (Jev) and review message",
                SettingsTab::Reviewer => "Settings: reviewer and setup checks",
                SettingsTab::Accounts => "Settings: GitHub accounts",
                SettingsTab::Appearance => "Settings: appearance and themes",
            }
            .into(),
            key: None,
            blocked: None,
            run: Run::Settings(tab),
        }
    }));
    out.extend([Ask::Start, Ask::Stop, Ask::Restart].map(|ask| Entry {
        group: "Reviewer",
        title: ask.title().into(),
        key: None,
        blocked: r.block(m, ask),
        run: Run::Reviewer(ask),
    }));
    out.extend(Choice::all().map(|c| Entry {
        group: "Appearance",
        title: format!("Theme: {}", c.name()),
        key: None,
        blocked: (c == theme).then(|| "in use".to_string()),
        run: Run::Theme(c),
    }));
    out
}

/// How well `query` matches: 0 a prefix of the title, 1 every word a prefix of a title word, 2 the initials of the
/// title's words, 3 a scattered subsequence. `None` when it does not match at all.
pub fn score(query: &str, title: &str) -> Option<u8> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let title_l = title.to_lowercase();
    let words: Vec<&str> = title_l.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let q_words: Vec<&str> = q.split_whitespace().collect();
    if title_l.starts_with(&q) {
        return Some(0);
    }
    if q_words.iter().all(|qw| words.iter().any(|w| w.starts_with(qw))) {
        return Some(1);
    }
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if !q.contains(' ') && initials.contains(&q) {
        return Some(2);
    }
    fuzzy(query, title).then_some(3)
}

/// The characters of `title` that `query` lit up, as byte ranges: each query character, in order, where it
/// continues the run before it, else at the next word start, else at its next place at all. Empty when the query
/// is empty or did not match the title itself.
pub fn matched(query: &str, title: &str) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut from = 0;
    for q in query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase) {
        let is = |c: char| c.to_lowercase().eq(std::iter::once(q));
        let rest: Vec<(usize, char)> = title[from..].char_indices().collect();
        let word_start = |k: usize| k == 0 && from == 0 || !title[..from + rest[k].0].ends_with(char::is_alphanumeric);
        let found = rest
            .first()
            .filter(|(_, c)| is(*c))
            .or_else(|| rest.iter().enumerate().find(|(k, (_, c))| is(*c) && word_start(*k)).map(|(_, x)| x))
            .or_else(|| rest.iter().find(|(_, c)| is(*c)));
        let Some(&(i, c)) = found else { return Vec::new() };
        let (a, b) = (from + i, from + i + c.len_utf8());
        match out.last_mut() {
            Some(last) if last.end == a => last.end = b,
            _ => out.push(a..b),
        }
        from = b;
    }
    // Bold reads only as one run or as word prefixes; letters lit across the middle of words read as noise.
    let starts_word = |r: &Range<usize>| !title[..r.start].ends_with(char::is_alphanumeric);
    if out.len() > 1 && !out.iter().all(starts_word) {
        return Vec::new();
    }
    out
}

/// The entries `query` matches, best first. Groups stay together: a group sorts by its best entry, and its entries
/// by their own score, runnable before blocked, then in the order `entries` gives.
fn matching(m: &Model, r: &Reviewer, theme: Choice, query: &str) -> Vec<Entry> {
    let found: Vec<(u8, usize, Entry)> = entries(m, r, theme)
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| score(query, &e.title).or_else(|| fuzzy(query, e.group).then_some(3)).map(|sc| (sc, i, e)))
        .collect();
    if query.trim().is_empty() {
        return found.into_iter().map(|(_, _, e)| e).collect();
    }
    let mut groups: Vec<(&'static str, u8, usize)> = Vec::new();
    for (sc, i, e) in &found {
        match groups.iter_mut().find(|g| g.0 == e.group) {
            Some(g) => g.1 = g.1.min(*sc),
            None => groups.push((e.group, *sc, *i)),
        }
    }
    let mut found = found;
    found.sort_by_key(|(sc, i, e)| {
        let g = groups.iter().find(|g| g.0 == e.group).map_or((3, 0), |g| (g.1, g.2));
        (g, *sc, e.blocked.is_some(), *i)
    });
    found.into_iter().map(|(_, _, e)| e).collect()
}

fn run(s: &mut Shell, r: Run, cx: &mut Context<Shell>) {
    s.overlay = None;
    match r {
        Run::Card(i) => s.select_card(i, cx),
        Run::Pr(id) => {
            s.done_tab = queue::done(&s.model).iter().any(|(p, _)| p.id == id);
            s.model.ui.show_archived = false;
            s.select_pr(id, cx)
        }
        Run::Direct(mu) => s.direct(mu, cx),
        Run::Act(a) => s.act(a, cx),
        Run::GitHub => s.open_github(cx),
        Run::Archive => s.archive(cx),
        Run::Tab(list) => s.set_tab(list, cx),
        Run::Usage => s.open_usage(None, cx),
        Run::Keys => s.overlay = Some(Overlay::Keys),
        Run::AddPr => s.add_pr(cx),
        Run::Reviewer(ask) => s.ask(ask, cx),
        Run::Theme(c) => s.set_theme(c, cx),
        Run::Settings(tab) => s.settings_on(tab, cx),
    }
    cx.notify();
}

/// Why the add-a-PR sheet can't send now, in the palette's and the buttons' words; `None` when it can.
/// A stopped reviewer the app can start doesn't block: the sheet starts it and sends the PR once it answers.
pub fn add_block(m: &Model, r: &Reviewer) -> Option<String> {
    if crate::reviewer::starts_for_add(m, r) {
        return None;
    }
    if queue::read_only(m) {
        return Some(READ_ONLY.into());
    }
    write_block(m, "review.add").map(str::to_string)
}

/// Keys while the palette is open; true when it took the key.
pub fn palette_key(s: &mut Shell, key: &str, typed: Option<&str>, cx: &mut Context<Shell>) -> bool {
    let Some(Overlay::Palette { query, selected }) = &s.overlay else { return false };
    let (mut q, mut sel) = (query.clone(), *selected);
    let n = matching(&s.model, &s.reviewer, crate::ui::chosen(cx), &q).len();
    match key {
        "escape" => s.overlay = None,
        "enter" => {
            if let Some(e) = matching(&s.model, &s.reviewer, crate::ui::chosen(cx), &q).into_iter().nth(sel)
                && e.blocked.is_none()
            {
                run(s, e.run, cx);
            }
            return true;
        }
        _ => {
            match key {
                "down" => sel = (sel + 1).min(n.saturating_sub(1)),
                "up" => sel = sel.saturating_sub(1),
                "backspace" => {
                    q.pop();
                    sel = 0;
                }
                _ => match typed {
                    Some(c) if !c.chars().any(char::is_control) => {
                        q.push_str(c);
                        sel = 0;
                    }
                    _ => return true,
                },
            }
            s.overlay = Some(Overlay::Palette { query: q, selected: sel });
        }
    }
    cx.notify();
    true
}

fn scrim(t: &Colors, id: &'static str, cx: &Context<Shell>, close: bool) -> Stateful<Div> {
    div().id(id).absolute().inset_0().bg(t.scrim).occlude().when(close, |d| {
        d.on_click(cx.listener(|s, _: &ClickEvent, _, cx| {
            s.overlay = None;
            cx.notify();
        }))
    })
}

fn sheet(t: &Colors, w: Pixels) -> Div {
    div()
        .w(w)
        .flex()
        .flex_col()
        .rounded(radius::LG)
        .border_1()
        .border_color(t.sheet_edge)
        .bg(t.surface)
        .overflow_hidden()
        .shadow(sheet_shadow(t))
}

/// A sheet's lift: a wide soft drop and a tight contact shadow, so it holds its edge over the scrim in dark too.
fn sheet_shadow(t: &Colors) -> Vec<BoxShadow> {
    let drop = |alpha: f32, y: f32, blur: f32| BoxShadow {
        color: t.shadow.alpha(alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    };
    vec![drop(t.shadow.a, 18., 48.), drop(t.shadow.a * 0.8, 2., 6.)]
}

/// A macOS sheet: it drops from under the title bar, its top edge flush there and its lower corners rounded.
fn drop_sheet(t: &Colors, w: Pixels) -> Div {
    sheet(t, w).rounded_t(px(0.)).border_t_0()
}

pub fn layer(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Vec<AnyElement> {
    let m = &s.model;
    let mut out = Vec::new();
    // A toast floats over the detail pane, above its action bar, so it never covers a card's button in the queue.
    let queue_shown = queue::stage(m) == Stage::Ready
        && matches!(m.ui.screen, Screen::Inbox | Screen::Review)
        && !matches!(crate::ui::front(m), Some(Modal::Finding { .. }));
    let left = layout::SIDEBAR + if queue_shown { layout::QUEUE } else { px(0.) };
    // The add-a-PR sheet says its own outcome in place; the same words in a toast under it would only repeat them.
    let adding = matches!(s.overlay, Some(Overlay::AddPr { .. }));
    if let Some(n) = m.notice.as_ref().filter(|_| !adding) {
        let (glyph, words) = match read_only_notice(m, &n.text) {
            Some(w) => (IconName::Lock, w),
            None => (IconName::Info, n.text.clone()),
        };
        out.push(toast(t, left, glyph, words));
    }
    match m.top_modal() {
        Some(Modal::Confirm { prompt, command, state }) => {
            // A start or retry in flight is a quiet status, not a sheet; a refusal or a question needs the reader.
            let quiet = matches!(command, Command::Start(_) | Command::Retry(_) | Command::StopDaemon(_))
                && matches!(state, ConfirmState::Requested(_) | ConfirmState::Accepted { .. });
            if quiet && matches!(command, Command::StopDaemon(_)) {
                out.push(toast(t, left, IconName::LoaderCircle, "Stopping the reviewer…".into()));
            } else if quiet {
                out.push(toast(t, left, IconName::LoaderCircle, format!("{} Sending…", prompt.trim_end_matches('?'))));
            } else {
                out.push(crate::kit::in_sheet(|| confirm(s, t, prompt, command, state, cx)));
            }
        }
        Some(Modal::Input { purpose, buffer, .. }) => out.push(input(t, *purpose, buffer, cx)),
        Some(Modal::Help { .. }) => out.push(keys(t, cx, true)),
        _ => {}
    }
    match &s.overlay {
        Some(Overlay::Palette { query, selected }) => out.push(palette(s, t, query, *selected, cx)),
        Some(Overlay::Keys) => out.push(keys(t, cx, false)),
        Some(Overlay::ConfirmPost(_)) => out.push(crate::kit::in_sheet(|| confirm_post(s, t, cx))),
        Some(Overlay::AddPr { url, sent }) => out.push(crate::kit::in_sheet(|| add_pr(s, t, url, *sent, cx))),
        Some(Overlay::ConfirmStop { restart, running }) => {
            out.push(crate::kit::in_sheet(|| confirm_stop(s, t, *restart, *running, cx)))
        }
        None => {}
    }
    out
}

/// The reducer's refusal of a command on a read-only daemon, in the desktop's words: what is off and how to turn
/// it on. `None` for any other notice.
pub fn read_only_notice(m: &Model, text: &str) -> Option<String> {
    if !queue::read_only(m) {
        return None;
    }
    let (verb, why) = text.strip_prefix("Can't ")?.split_once(": ")?;
    let off = m.server.as_ref().and_then(|s| s.commands_off());
    if Some(why) != off && why != "the daemon does not grant it" {
        return None;
    }
    let what = match verb {
        "start" => "starting reviews",
        "retry" => "retrying",
        "cancel" => "canceling",
        "publish" => "posting",
        "add a PR" => "adding PRs",
        _ => "commands",
    };
    let fix = if m.server.as_ref().is_some_and(|s| s.missing().next().is_some()) {
        "Fix setup, then restart it with --allow-commands."
    } else {
        "Restart it with --allow-commands."
    };
    Some(format!("Read-only daemon: {what} is off. {fix}"))
}

/// A notice over the detail pane, 16 points above its action bar: a raised surface with the sheet's edge, never a
/// bright pill.
fn toast(t: &Colors, left: Pixels, glyph: IconName, words: String) -> AnyElement {
    div()
        .absolute()
        .bottom(layout::BAR + space::S4)
        .left(left)
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .px(space::S4)
                .h(px(36.))
                .rounded(radius::PILL)
                .bg(t.surface)
                .border_1()
                .border_color(t.sheet_edge)
                .shadow(sheet_shadow(t))
                .text_color(t.text)
                .text_size(text::BODY)
                .child(icon(glyph, px(14.), t.muted))
                .child(words),
        )
        .into_any_element()
}

/// A palette row's glyph, so an action, a place to go and a view read apart before their words do.
fn glyph(m: &Model, e: &Entry) -> IconName {
    match &e.run {
        Run::Card(i) => match queue::cards(m).get(*i) {
            Some(Card::Setup { .. }) => IconName::TriangleAlert,
            Some(Card::Approve { .. }) => IconName::Send,
            Some(Card::Posting { .. }) => IconName::CircleAlert,
            Some(Card::Findings { .. }) => IconName::Eye,
            Some(Card::Failed { .. }) => IconName::RotateCw,
            Some(Card::Start { .. }) | None => IconName::Play,
        },
        Run::Pr(_) if e.group == "Go to" => IconName::GitPullRequest,
        Run::Pr(_) => IconName::Activity,
        Run::Direct(Mutation::Start) => IconName::Play,
        Run::Direct(_) => IconName::RotateCw,
        Run::Act(Action::Mutation(Mutation::Publish)) => IconName::Send,
        Run::Act(Action::Mutation(_)) => IconName::CircleX,
        Run::Act(Action::OpenDiff) => IconName::FileDiff,
        Run::Act(Action::OpenTimeline) => IconName::Clock,
        Run::Act(Action::Refresh) => IconName::RefreshCw,
        Run::Act(_) => IconName::Settings,
        Run::GitHub => IconName::ExternalLink,
        Run::Archive => IconName::Archive,
        Run::Tab(List::Queue) => IconName::Inbox,
        Run::Tab(List::Done) => IconName::CircleCheck,
        Run::Tab(List::Archived) => IconName::Archive,
        Run::Usage => IconName::ChartColumn,
        Run::Keys => IconName::Keyboard,
        Run::AddPr => IconName::Plus,
        Run::Reviewer(Ask::Start) => IconName::Power,
        Run::Reviewer(Ask::Stop) => IconName::CircleStop,
        Run::Reviewer(Ask::Restart) => IconName::RotateCcw,
        Run::Theme(_) => IconName::Palette,
        Run::Settings(_) => IconName::Settings,
    }
}

/// The list's height: rows are 36 points, a group header 30, inside 8 of padding. It never ends mid-row: at most
/// the whole rows that fit in 440 points, and a fade says the rest scrolls under.
const ROW: f32 = 36.;
const HEADER: f32 = 30.;

fn palette(s: &Shell, t: &Colors, query: &str, selected: usize, cx: &Context<Shell>) -> AnyElement {
    let m = &s.model;
    let found = matching(m, &s.reviewer, crate::ui::chosen(cx), query);
    let mut list = div().id("palette-list").flex().flex_col().overflow_y_scroll().p(space::S2);
    let mut group = "";
    let (mut tall, mut fits, mut more) = (8., 8., false);
    if found.is_empty() {
        list = list.child(
            div()
                .p(space::S4)
                .text_size(text::BODY)
                .text_color(t.muted)
                .child(format!("Nothing matches \u{201c}{query}\u{201d}")),
        );
        fits = 60.;
    }
    for (i, e) in found.into_iter().enumerate() {
        if e.group != group {
            group = e.group;
            tall += HEADER;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(HEADER))
                    .px(space::S3)
                    .pt(space::S2)
                    .child(overline(t, group)),
            );
        }
        tall += ROW;
        if tall <= 440. {
            fits = tall;
        } else {
            more = true;
        }
        let on_it = i == selected;
        let blocked = e.blocked.is_some();
        let r = e.run.clone();
        // A row that can't run is dimmed to about half, as one opaque color that still meets AA. The chosen row is
        // the selection: its fill and the accent rail, with its text at full strength.
        let fg = if blocked { t.disabled } else { t.text };
        let hits: Vec<_> = matched(query, &e.title)
            .into_iter()
            .map(|r| (r, HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() }))
            .collect();
        let mark = glyph(m, &e);
        let locked = e.blocked.as_deref() == Some(READ_ONLY);
        let why = e.blocked;
        let row = div()
            .id(("pal", i))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S3)
            .h(px(ROW))
            .px(space::S3)
            .rounded(radius::SM)
            .cursor_pointer()
            .text_color(fg)
            .when(on_it, |d| d.bg(t.selected).child(crate::kit::rail(t)))
            // The pointer moves the one selection, so a hovered row and the chosen row are never two rows.
            .on_hover(cx.listener(move |s, over: &bool, _, cx| {
                if let (true, Some(Overlay::Palette { selected, .. })) = (*over, &mut s.overlay)
                    && *selected != i
                {
                    *selected = i;
                    cx.notify();
                }
            }))
            .when(!blocked, |d| d.on_click(cx.listener(move |s, _: &ClickEvent, _, cx| run(s, r.clone(), cx))))
            .child(icon(
                mark,
                px(14.),
                if blocked {
                    t.disabled
                } else if on_it {
                    t.accent
                } else {
                    t.muted
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(text::BODY)
                    .child(StyledText::new(e.title).with_highlights(hits)),
            )
            .children(why.map(|w| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(5.))
                    .pr(space::S4)
                    .text_size(text::CAPTION)
                    .when(locked, |d| d.child(icon(IconName::Lock, px(11.), t.disabled)))
                    .child(w)
            }))
            .child(match e.key {
                Some(k) => keycap(t, k, None, blocked),
                None => div().w(px(18.)),
            })
            .when(on_it && !blocked, |d| d.child(keycap(t, "⏎", None, false)));
        list = list.child(if locked { tip(row, "Restart the daemon with --allow-commands to run it.") } else { row });
    }
    let list = div().relative().h(px(if more { fits } else { fits + 8. })).child(list.size_full()).when(more, |d| {
        d.child(div().absolute().bottom_0().left_0().right_0().h(space::S6).bg(linear_gradient(
            180.,
            linear_color_stop(t.surface.alpha(0.), 0.),
            linear_color_stop(t.surface, 1.),
        )))
    });
    scrim(t, "palette-scrim", cx, true)
        .flex()
        .items_start()
        .justify_center()
        .pt(px(96.))
        .child(
            sheet(t, layout::PALETTE)
                .id("palette")
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .h(px(52.))
                        .px(space::S4)
                        .border_b_1()
                        .border_color(t.border)
                        .child(icon(IconName::Search, px(16.), t.muted))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .flex_1()
                                .text_size(text::LEAD)
                                .child(if query.is_empty() {
                                    div().text_color(t.faint).child("Type a command, a PR number or a title")
                                } else {
                                    div().text_color(t.text).child(query.to_string())
                                })
                                .child(div().w(px(2.)).h(px(18.)).ml(px(1.)).bg(t.accent)),
                        )
                        .child(keycap(t, "esc", None, false)),
                )
                .child(list)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S4)
                        .h(px(36.))
                        .px(space::S4)
                        .border_t_1()
                        .border_color(t.border)
                        .bg(t.raised)
                        .text_size(text::CAPTION)
                        .text_color(t.faint)
                        .children([("↑↓", "choose"), ("⏎", "run"), ("esc", "close")].map(|(k, what)| {
                            div().flex().items_center().gap(px(6.)).child(keycap(t, k, None, false)).child(what)
                        }))
                        .child(div().flex_1())
                        .child("dimmed actions say why they can't run"),
                ),
        )
        .into_any_element()
}

/// The reducer's confirmation: a cancel asks with Keep as the default; a refused start or retry explains.
fn confirm(
    s: &Shell,
    t: &Colors,
    prompt: &str,
    command: &Command,
    state: &ConfirmState,
    cx: &Context<Shell>,
) -> AnyElement {
    let (verb, tone, glyph, hue) = match command {
        Command::Start(_) => ("Start review", Tone::Primary, IconName::Play, t.accent),
        Command::Retry(_) => ("Retry", Tone::Primary, IconName::RotateCw, t.accent),
        Command::Again(_) => ("Review again", Tone::Primary, IconName::RotateCw, t.accent),
        Command::Cancel(_) => ("Cancel review", Tone::Destructive, IconName::CircleX, t.danger),
        Command::SelectAccount(_) => ("Use account", Tone::Primary, IconName::User, t.accent),
        Command::StopDaemon(_) => ("Stop reviewer", Tone::Destructive, IconName::CircleStop, t.danger),
        _ => ("Confirm", Tone::Primary, IconName::Check, t.accent),
    };
    let status = match state {
        ConfirmState::Asking => None,
        ConfirmState::Requested(_) => Some(("Sending…".to_string(), t.running)),
        ConfirmState::Accepted { .. } => {
            Some(("Accepted. Waiting for the daemon to report it.".to_string(), t.running))
        }
        ConfirmState::Refused { message, .. } => Some((message.to_string(), t.danger)),
        ConfirmState::TimedOut => Some(("No reply in time. ⏎ sends it again under the same key.".to_string(), t.warn)),
    };
    let ready = can_confirm(&s.model);
    let done = matches!(state, ConfirmState::Accepted { .. } | ConfirmState::Refused { .. });
    let keep = matches!(command, Command::Cancel(_));
    scrim(t, "confirm-scrim", cx, false)
        .flex()
        .items_start()
        .justify_center()
        .pt(layout::TOOLBAR)
        .child(
            drop_sheet(t, layout::SHEET)
                .p(space::S6)
                .gap(space::S4)
                .child(
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::PILL)
                        .bg(hue.alpha(0.14))
                        .child(icon(glyph, px(18.), hue)),
                )
                .child(
                    div()
                        .text_size(text::TITLE)
                        .line_height(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(prompt.to_string()),
                )
                .child(div().text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(
                    match command {
                        Command::Cancel(_) => {
                            "The run stops where it is and its work so far is lost. Nothing it found is posted."
                        }
                        Command::Start(_) | Command::Retry(_) => {
                            "The reviewer reads the diff with read-only tools. Nothing posts without your approval."
                        }
                        Command::Again(_) => {
                            "A new run reviews this head from scratch with the current model. A proof waiting for \
                             approval is dropped unposted; findings found again stay the same findings."
                        }
                        Command::StopDaemon(_) => {
                            "The reviewer shuts down cleanly. Running reviews pause and resume on the next start."
                        }
                        _ => "The daemon uses this account for GitHub reads and for posting.",
                    },
                ))
                .children(status.map(|(s, c)| div().text_size(text::BODY).text_color(c).child(s)))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(button(
                            t,
                            "c-no",
                            if done {
                                "Close"
                            } else if keep {
                                "Keep running"
                            } else {
                                "Not now"
                            },
                            Some("esc"),
                            if keep && !done { Tone::Secondary } else { Tone::Ghost },
                            on(cx, |s, cx| s.act(Action::Back, cx)),
                        ))
                        .when(!done, |d| {
                            d.child(button(
                                t,
                                "c-yes",
                                verb,
                                Some(if keep { "⌘⏎" } else { "⏎" }),
                                tone,
                                ready.then(|| -> crate::kit::OnClick {
                                    Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::Confirm, cx)))
                                }),
                            ))
                        }),
                ),
        )
        .into_any_element()
}

/// The one irreversible act asks once more, naming exactly what goes out. Cancel is the default, on esc and on ⏎;
/// only ⌘⏎ posts.
fn confirm_post(s: &Shell, t: &Colors, cx: &Context<Shell>) -> AnyElement {
    let m = &s.model;
    let Some(Modal::Publication { run_id, preview: Preview::Ready(p), .. }) = m.top_modal() else {
        return div().into_any_element();
    };
    let pr = m.domain.runs.get(run_id).and_then(|r| m.domain.prs.get(&r.pr_id));
    let target = pr.map_or("this PR".to_string(), |p| format!("{} #{}", p.repository, p.number));
    let fact = |label: &str, value: String| {
        div()
            .flex()
            .gap(space::S3)
            .text_size(text::BODY)
            .child(div().w(px(96.)).flex_none().text_color(t.faint).child(label.to_string()))
            .child(div().flex_1().min_w_0().text_color(t.text).child(value))
    };
    scrim(t, "post-scrim", cx, true)
        .flex()
        .items_start()
        .justify_center()
        .pt(layout::TOOLBAR)
        .child(
            drop_sheet(t, layout::SHEET)
                .id("post-sheet")
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .p(space::S6)
                .gap(space::S4)
                .child(
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::PILL)
                        .bg(t.accent.alpha(0.14))
                        .child(icon(IconName::Send, px(18.), t.accent)),
                )
                .child(
                    div()
                        .text_size(text::TITLE)
                        .line_height(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Post this review to {target}?")),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::S2)
                        .child(fact("As", who(m)))
                        .child(fact("At commit", p.head_sha.short().to_string()))
                        .child(fact(
                            "It posts",
                            format!("1 review body and {}", queue::count(p.comments.len(), "inline comment")),
                        ))
                        .child(fact("Review kind", p.event.as_str().to_lowercase().replace('_', " "))),
                )
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(space::S2)
                        .child(div().pt(px(3.)).child(icon(IconName::TriangleAlert, px(14.), t.warn)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(text::BODY)
                                .line_height(text::LINE_BODY)
                                .text_color(t.text)
                                .child("This can't be undone from here. GitHub keeps the review once it is posted."),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(button(
                            t,
                            "post-no",
                            "Cancel",
                            Some("esc"),
                            Tone::Secondary,
                            on(cx, |s, cx| {
                                s.overlay = None;
                                cx.notify();
                            }),
                        ))
                        .child(button(
                            t,
                            "post-yes",
                            "Post to GitHub",
                            Some("⌘⏎"),
                            Tone::Primary,
                            on(cx, |s, cx| s.confirm_post(cx)),
                        )),
                ),
        )
        .into_any_element()
}

/// What the add-a-PR sheet says under its field: what the text names, or how the daemon answered it.
#[derive(Debug, PartialEq)]
pub enum AddLine {
    /// Nothing typed yet.
    Empty,
    /// Text that names no PR yet.
    Invalid,
    /// `repo · #N`, and the title when the window already tracks it.
    Valid {
        repo: String,
        number: u64,
        tracked: Option<String>,
    },
    Sending(String),
    /// Sent while the reviewer was stopped: the app is starting it, and the PR goes once it answers.
    Starting(String),
    Refused {
        title: &'static str,
        why: String,
    },
    TimedOut,
}

pub fn add_line(m: &Model, url: &str, sent: bool) -> AddLine {
    let Some(target) = PrRef::parse(url) else {
        return if url.trim().is_empty() { AddLine::Empty } else { AddLine::Invalid };
    };
    match m.adding.as_ref().filter(|a| sent && a.target == target).map(|a| &a.state) {
        Some(AddState::Sending(_) | AddState::Landing { .. }) => AddLine::Sending(target.to_string()),
        Some(AddState::TimedOut) => AddLine::TimedOut,
        Some(AddState::Refused { code, message }) => {
            let title = match code {
                Some(ErrorCode::NotFound) => "GitHub won't show this PR to the daemon",
                Some(ErrorCode::Conflict) => "This PR can't be reviewed",
                Some(ErrorCode::InvalidRequest) => "That isn't a GitHub pull request",
                Some(ErrorCode::TemporarilyUnavailable | ErrorCode::RateLimited) => "GitHub didn't answer",
                Some(ErrorCode::CapabilityDenied) => "This daemon can't add PRs",
                _ => "The daemon couldn't add it",
            };
            AddLine::Refused { title, why: crate::kit::sentence(message.as_str()) }
        }
        None => AddLine::Valid {
            number: target.number,
            tracked: m.domain.prs.values().find(|p| target.matches(p)).map(|p| p.title.to_string()),
            repo: target.repository,
        },
    }
}

/// The sheet that adds a PR: one field for its URL, read as it is typed, and the daemon's answer in place. It drops
/// from the title bar as the post sheet does; esc closes it and ⏎ sends.
fn add_pr(s: &Shell, t: &Colors, url: &str, sent: bool, cx: &Context<Shell>) -> AnyElement {
    let m = &s.model;
    let line = match (&s.queued_add, &s.reviewer.control) {
        (Some(_), crate::reviewer::Control::StartFailed(why)) if sent => {
            AddLine::Refused { title: "The reviewer didn't start", why: why.clone() }
        }
        (Some(q), _) if sent => AddLine::Starting(PrRef::parse(q).map_or(q.clone(), |r| r.to_string())),
        _ => add_line(m, url, sent),
    };
    let sending = matches!(line, AddLine::Sending(_) | AddLine::Starting(_));
    let ready = matches!(line, AddLine::Valid { .. } | AddLine::Refused { .. } | AddLine::TimedOut) && !sending;
    let (glyph, hue, words): (Option<IconName>, Hsla, String) = match &line {
        AddLine::Empty => (None, t.faint, "Paste a link with ⌘V. owner/repo#123 works too.".into()),
        AddLine::Invalid => (
            Some(IconName::CircleAlert),
            t.warn,
            "Not a pull request yet: github.com/owner/repo/pull/123 or owner/repo#123.".into(),
        ),
        AddLine::Valid { repo, number, tracked: None } => {
            (Some(IconName::GitPullRequest), t.ok, format!("{repo} · #{number}"))
        }
        AddLine::Valid { repo, number, tracked: Some(title) } => {
            (Some(IconName::CircleCheck), t.ok, format!("{repo} · #{number} is already here: {title}. ⏎ opens it."))
        }
        AddLine::Sending(target) => (None, t.running, format!("Asking the daemon to read {target} from GitHub…")),
        AddLine::Starting(target) => (None, t.running, format!("Starting the reviewer, then adding {target}…")),
        AddLine::Refused { why, .. } => (Some(IconName::CircleX), t.danger, why.clone()),
        AddLine::TimedOut => (Some(IconName::TriangleAlert), t.warn, "No reply in time. ⏎ sends it again.".into()),
    };
    let status = div()
        .flex()
        .items_start()
        .gap(space::S2)
        .min_h(px(21.))
        .when(glyph.is_some() || sending, |d| {
            d.child(div().pt(px(3.)).flex_none().child(match glyph {
                Some(g) => icon(g, px(14.), hue).into_any_element(),
                None => crate::setup::spinner(t, s.still, "add-spin"),
            }))
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .text_size(text::BODY)
                .line_height(text::LINE_BODY)
                .children(match &line {
                    AddLine::Refused { title, .. } => {
                        Some(div().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(*title))
                    }
                    _ => None,
                })
                .child(
                    div().text_color(if matches!(line, AddLine::Refused { .. }) { t.muted } else { hue }).child(words),
                ),
        );
    let field = div()
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(40.))
        .px(space::S3)
        .rounded(radius::SM)
        .bg(t.bg)
        .border_1()
        .border_color(match line {
            AddLine::Invalid => t.warn,
            AddLine::Refused { .. } => t.danger,
            _ => t.accent,
        })
        .child(icon(IconName::GitPullRequest, px(15.), t.muted))
        .child(
            div()
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .font_family(text::MONO)
                .text_size(text::BODY)
                // The caret sits where typing lands: before the placeholder, after the text.
                .when(url.is_empty(), |d| {
                    d.child(caret(t)).child(div().text_color(t.faint).child("https://github.com/owner/repo/pull/123"))
                })
                .when(!url.is_empty(), |d| {
                    d.child(div().truncate().text_color(t.text).child(url.to_string()))
                        .when(!sending, |d| d.child(caret(t)))
                }),
        );
    scrim(t, "add-scrim", cx, false)
        .flex()
        .items_start()
        .justify_center()
        .pt(layout::TOOLBAR)
        .child(
            drop_sheet(t, layout::SHEET)
                .id("add-sheet")
                .occlude()
                .p(space::S6)
                .gap(space::S4)
                .child(
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::PILL)
                        .bg(t.accent.alpha(0.14))
                        .child(icon(IconName::Plus, px(18.), t.accent)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::S1)
                        .child(
                            div()
                                .text_size(text::TITLE)
                                .line_height(px(24.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Review a PR"),
                        )
                        .child(div().text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(
                            // Only a daemon that parks for approval grants review.publish.
                            if m.may_write("review.publish") {
                                "The daemon reads it from GitHub with its own account and starts the review. Nothing posts without your approval."
                            } else {
                                "The daemon reads it from GitHub with its own account and starts the review."
                            },
                        )),
                )
                .child(div().flex().flex_col().gap(space::S2).child(field).child(status))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(button(t, "add-no", "Cancel", Some("esc"), Tone::Secondary, on(cx, |s, cx| s.close_add(cx))))
                        .child(button(
                            t,
                            "add-yes",
                            if sending { "Adding…" } else { "Review PR" },
                            Some("⏎"),
                            Tone::Primary,
                            ready.then(|| -> crate::kit::OnClick {
                                Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.submit_add(cx)))
                            }),
                        )),
                ),
        )
        .into_any_element()
}

fn caret(t: &Colors) -> Div {
    div().flex_none().w(px(2.)).h(px(18.)).mx(px(1.)).bg(t.accent)
}

/// A stop asks first when it pauses running reviews, or when launchd would start the daemon again at once: then it
/// unloads the LaunchAgent instead, so the reviewer stays stopped. Keep running is the default, on esc and on ⏎.
fn confirm_stop(s: &Shell, t: &Colors, restart: bool, running: usize, cx: &Context<Shell>) -> AnyElement {
    let relaunches = s.reviewer.launcher.as_ref().is_ok_and(crate::reviewer::Launcher::relaunches);
    let (title, verb) = match (restart, relaunches) {
        (true, _) => ("Restart the reviewer?", "Restart reviewer"),
        (false, true) => ("Stop the reviewer and keep it stopped?", "Stop and keep stopped"),
        (false, false) => ("Stop the reviewer?", "Stop reviewer"),
    };
    let mut lines = Vec::new();
    if running > 0 {
        lines.push(format!(
            "{} {} now and {} where {} left off when the reviewer starts again. Nothing is lost.",
            queue::one(running),
            if running == 1 { "review is running. It pauses" } else { "reviews are running. They pause" },
            if running == 1 { "resumes" } else { "resume" },
            if running == 1 { "it" } else { "they" },
        ));
    }
    if relaunches && !restart {
        lines.push(
            "Its LaunchAgent sets KeepAlive to true, so launchd would start it again at once. This unloads the \
             job with launchctl bootout instead, and Start reviewer loads it again."
                .into(),
        );
    }
    scrim(t, "stop-scrim", cx, true)
        .flex()
        .items_start()
        .justify_center()
        .pt(layout::TOOLBAR)
        .child(
            drop_sheet(t, layout::SHEET)
                .id("stop-sheet")
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .p(space::S6)
                .gap(space::S4)
                .child(
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::PILL)
                        .bg(t.danger.alpha(0.14))
                        .child(icon(IconName::CircleStop, px(18.), t.danger)),
                )
                .child(div().text_size(text::TITLE).line_height(px(24.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .children(
                    lines
                        .into_iter()
                        .map(|l| div().text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(l)),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(button(
                            t,
                            "stop-no",
                            "Keep running",
                            Some("esc"),
                            Tone::Secondary,
                            on(cx, |s, cx| {
                                s.overlay = None;
                                cx.notify();
                            }),
                        ))
                        .child(button(
                            t,
                            "stop-yes",
                            verb,
                            Some("⌘⏎"),
                            Tone::Destructive,
                            on(cx, |s, cx| s.confirm_stop(cx)),
                        )),
                ),
        )
        .into_any_element()
}

fn input(t: &Colors, purpose: InputPurpose, buffer: &str, cx: &Context<Shell>) -> AnyElement {
    let label = match purpose {
        InputPurpose::JumpTo => "Jump to path:line",
        InputPurpose::DiffSearch => "Search the diff",
        InputPurpose::TimelineFilter => "Filter events",
        InputPurpose::Search => "Filter PRs",
        InputPurpose::AddPr => "Add a PR",
    };
    scrim(t, "input-scrim", cx, false)
        .flex()
        .items_start()
        .justify_center()
        .pt(px(96.))
        .child(
            sheet(t, layout::PALETTE).child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .h(px(52.))
                    .px(space::S4)
                    .child(div().text_size(text::SMALL).text_color(t.muted).child(label))
                    .child(div().flex_1().font_family(text::MONO).text_size(text::LEAD).child(buffer.to_string()))
                    .child(div().w(px(2.)).h(px(18.)).bg(t.accent))
                    .child(keycap(t, "⏎", None, false)),
            ),
        )
        .into_any_element()
}

const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Queue",
        &[
            ("J / K", "next or previous card"),
            ("⏎", "do what the card's button says"),
            ("N / ⇧N", "next or previous finding in the PR"),
            ("D", "open the diff at the finding"),
            ("T", "the PR's timeline, and back to its findings"),
            ("⇥", "move between controls"),
            ("⌘[  esc", "back"),
        ],
    ),
    (
        "Decide",
        &[
            ("P", "open the proof of what would post"),
            ("⌘⏎", "post the proof, or cancel a review, after its sheet"),
            ("S", "start a review (no sheet)"),
            ("⇧R", "retry a failed review (no sheet)"),
            ("⇧S", "review the PR again from scratch"),
            ("C", "cancel a review (asks first)"),
            ("E", "archive the PR, or unarchive it"),
        ],
    ),
    (
        "Anywhere",
        &[
            ("⌘K  /", "command palette"),
            ("⌘N", "review a PR by its URL"),
            ("⌘1 ⌘2 ⌘3 ⌘4", "needs you, done, archived, usage"),
            ("D  W  M", "usage by day, week, month"),
            ("T  R  A  P", "usage over time, by repository, author, PR"),
            ("⌘,", "settings, on the tab you left"),
            ("1 2 3 4", "settings: review, reviewer, accounts, appearance"),
            ("⌘⇧O", "open on GitHub"),
            ("⌘R", "refresh"),
            ("⌘W", "close the window"),
            ("?", "this sheet"),
        ],
    ),
];

fn keys(t: &Colors, cx: &Context<Shell>, reducer: bool) -> AnyElement {
    let groups = div().flex().gap(space::S6).children(KEYS.iter().map(|(g, rows)| {
        div().flex().flex_col().gap(space::S2).flex_1().child(overline(t, *g)).children(rows.iter().map(|(k, what)| {
            div()
                .flex()
                .items_center()
                .gap(space::S3)
                .child(div().w(px(64.)).flex().child(keycap(t, k, None, false)))
                .child(div().text_size(text::SMALL).text_color(t.muted).child(what.to_string()))
        }))
    }));
    let s = scrim(t, "keys-scrim", cx, !reducer);
    let s = if reducer { s.on_click(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::Back, cx))) } else { s };
    s.flex()
        .items_center()
        .justify_center()
        .child(
            sheet(t, px(780.))
                .p(space::S6)
                .gap(space::S5)
                .child(div().text_size(text::TITLE).font_weight(FontWeight::SEMIBOLD).child("Keyboard shortcuts"))
                .child(groups),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn matched_lights_the_query_in_order() {
        assert_eq!(super::matched("re", "Review and post #1291"), vec![0..2]);
        assert_eq!(super::matched("rtf", "Retry the failed review"), [0..1, 6..7, 10..11]);
        assert_eq!(super::matched("zz", "Retry"), Vec::<std::ops::Range<usize>>::new());
        assert_eq!(super::matched("", "Retry"), Vec::<std::ops::Range<usize>>::new());
        // "start" scattered through "Orders: self-serve refunds, stock holds at…" lights nothing.
        assert_eq!(
            super::matched("start", "#1287 Orders: self-serve refunds, stock holds at checkout"),
            Vec::<std::ops::Range<usize>>::new()
        );
        assert_eq!(super::matched("start", "Start a review of #1296"), vec![0..5]);
    }

    #[test]
    fn a_search_for_a_setting_opens_settings_on_the_tab_that_holds_it() {
        use super::{Run, SettingsTab as Tab};
        let m =
            pr_review_tui::model::Model::new("test", Default::default(), pr_review_tui::model::Screen::Review, None);
        let r = crate::reviewer::Reviewer::new(None, Default::default(), "config.toml".into(), true);
        for (query, tab) in [
            ("model", Tab::Review),
            ("jev", Tab::Review),
            ("review message", Tab::Review),
            ("setup checks", Tab::Reviewer),
            ("accounts", Tab::Accounts),
            ("themes", Tab::Appearance),
        ] {
            let best = super::matching(&m, &r, crate::tokens::Choice::System, query)
                .into_iter()
                .find(|e| matches!(e.run, Run::Settings(_)))
                .unwrap_or_else(|| panic!("{query}: no Settings row"));
            assert!(matches!(best.run, Run::Settings(t) if t == tab), "{query} opens {:?}: {}", tab, best.title);
        }
    }

    #[test]
    fn a_read_only_daemon_dims_every_command_row_with_its_reason_but_archiving_which_it_grants() {
        use pr_review_tui::model::{Look, Model, Screen};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/playback/showcase.ndjson");
        let mut m = Model::new("test", Look::default(), Screen::Review, None);
        let steps = crate::script::parse("wait:19000,pr:1287").unwrap();
        crate::script::play(pr_review_tui::playback::load(&path).unwrap(), &mut m, &steps, 20_000).unwrap();
        // What a read-only daemon grants: the reader's own commands, none of the review ones.
        m.server.as_mut().unwrap().capabilities =
            ["account.select", "pr.archive", "pr.unarchive"].map(pr_review_tui::protocol::Text::new).to_vec();
        let r = crate::reviewer::Reviewer::new(None, Default::default(), "config.toml".into(), true);
        let rows: Vec<(String, Option<String>)> = super::entries(&m, &r, crate::tokens::Choice::System)
            .into_iter()
            .filter(|e| e.group == "Review")
            .map(|e| (e.title, e.blocked))
            .collect();
        let locked = Some(super::READ_ONLY.to_string());
        assert_eq!(
            rows,
            [
                ("Review and post the proof of #1287".to_string(), locked.clone()),
                ("Start a review of #1287 at its head".to_string(), locked.clone()),
                ("Review #1287 again from scratch".to_string(), locked.clone()),
                ("Retry the failed review of #1287".to_string(), locked.clone()),
                ("Cancel the review of #1287".to_string(), locked.clone()),
                ("Review a PR by its URL…".to_string(), locked),
                ("Archive #1287".to_string(), None),
                ("Open #1287 on GitHub".to_string(), None),
            ]
        );
    }

    #[test]
    fn the_add_sheet_reads_the_url_as_typed_and_shows_the_daemons_refusal_in_place() {
        use super::{AddLine, add_line};
        use pr_review_tui::model::{Look, Model, Screen};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/playback/showcase.ndjson");
        let play = |steps: &str| {
            let mut m = Model::new("test", Look::default(), Screen::Review, None);
            let steps = crate::script::parse(steps).unwrap();
            crate::script::play(pr_review_tui::playback::load(&path).unwrap(), &mut m, &steps, 20_000).unwrap();
            m
        };
        let m = play("wait:19000");
        assert_eq!(add_line(&m, " ", false), AddLine::Empty);
        assert_eq!(add_line(&m, "https://github.com/acme", false), AddLine::Invalid);
        assert_eq!(
            add_line(&m, "https://github.com/acme/web/pull/77/files", false),
            AddLine::Valid { repo: "acme/web".into(), number: 77, tracked: None }
        );
        assert!(matches!(add_line(&m, "acme/shop#1287", false), AddLine::Valid { tracked: Some(_), .. }));
        let m = play("wait:19000,add:acme/web#77");
        let AddLine::Refused { title, why } = add_line(&m, "acme/web#77", true) else { panic!("{:?}", m.adding) };
        assert_eq!(title, "GitHub won't show this PR to the daemon");
        assert!(why.starts_with("Cannot read acme/web#77"), "{why}");
        assert!(matches!(add_line(&m, "acme/web#78", true), AddLine::Valid { .. }), "an edited URL is a new question");
    }

    #[test]
    fn prefixes_and_initials_rank_before_subsequences() {
        assert_eq!(score("retry", "Review Retry the failed review of #345"), Some(1));
        assert_eq!(score("review retry", "Review Retry the failed review of #345"), Some(0));
        assert_eq!(score("rtf", "Review Retry the failed review"), Some(2));
        assert_eq!(score("rvw", "Review Retry the failed review"), Some(3));
        assert_eq!(score("zzz", "Review Retry"), None);
        assert_eq!(score("", "anything"), Some(0));
    }
}
