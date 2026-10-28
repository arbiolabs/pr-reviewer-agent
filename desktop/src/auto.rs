//! Settings → Reviewer → Automatic reviews: whether the daemon starts reviews on its own for the PRs that request
//! the account's review, how often it asks GitHub, how many a day, which repositories count, and what its last poll
//! did.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::model::SaveState;
use pr_review_tui::protocol::{AccessibleRepo, AutoReview, AutoReviewParams, RepoParams};
use pr_review_tui::update::{Msg, write_block};

use crate::kit::{icon, segmented};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::Shell;

const INTERVALS: [(u32, &str); 3] = [(5, "5 min"), (15, "15 min"), (30, "30 min")];
const CAPS: [(u32, &str); 5] = [(10, "10"), (20, "20"), (50, "50"), (100, "100"), (0, "No cap")];
/// The dropdown lists this many matches at most; typing narrows the rest in.
const MENU_ROWS: usize = 50;
const MENU_HEIGHT: Pixels = px(264.);

/// The repository list a combobox edits: `include` (Only) or `exclude` (Never).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Only,
    Never,
}

impl Field {
    pub const BOTH: [Field; 2] = [Field::Only, Field::Never];

    fn other(self) -> Field {
        match self {
            Field::Only => Field::Never,
            Field::Never => Field::Only,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Field::Only => "only",
            Field::Never => "never",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Field::Only => "Only",
            Field::Never => "Never",
        }
    }
}

/// Both lists, indexed by `Field`: `[include, exclude]`.
pub type Lists = [Vec<String>; 2];

/// One combobox: its search field and whether its dropdown is open, on which row.
#[derive(Default)]
pub struct Combo {
    pub input: Option<Entity<InputState>>,
    open: bool,
    highlight: Option<usize>,
    /// Why ⏎ added nothing, until the next keystroke.
    problem: Option<String>,
    _events: Option<Subscription>,
}

/// The two repository comboboxes. Each change saves at once: one settings.set is in flight at a time and each
/// carries both lists whole, so a change made while one is in flight waits in `want` and the daemon ends up with the
/// last lists the reader made, whatever order the answers come in.
#[derive(Default)]
pub struct RepoFields {
    pub only: Combo,
    pub never: Combo,
    want: Option<Lists>,
    /// Sent and not yet read back: shown until the settings that follow the save arrive, or the save fails.
    sent: Option<Lists>,
    /// `repos:FIELD:QUERY`: the snapshot harness's dropdown, open with QUERY typed.
    pub staged: Option<(Field, String)>,
}

impl RepoFields {
    fn combo(&self, f: Field) -> &Combo {
        match f {
            Field::Only => &self.only,
            Field::Never => &self.never,
        }
    }

    fn combo_mut(&mut self, f: Field) -> &mut Combo {
        match f {
            Field::Only => &mut self.only,
            Field::Never => &mut self.never,
        }
    }

    pub fn inputs(&self) -> impl Iterator<Item = &Entity<InputState>> {
        [self.only.input.as_ref(), self.never.input.as_ref()].into_iter().flatten()
    }
}

/// `owner/name`, each part letters, digits, `_`, `.` or `-`.
pub fn valid(name: &str) -> bool {
    let ok = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    matches!(name.split_once('/'), Some((o, r)) if ok(o) && ok(r))
}

fn has(list: &[String], name: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(name))
}

/// The query's characters appear in `name` in order, ignoring case and spaces.
fn fuzzy(name: &str, query: &str) -> bool {
    let mut rest = name.chars().map(|c| c.to_ascii_lowercase());
    query.chars().filter(|c| !c.is_whitespace()).all(|q| rest.any(|c| c == q.to_ascii_lowercase()))
}

/// A dropdown row: a repository the account reaches, or (`typed`) the `owner/name` typed when no row names it.
/// `taken` is the other list when the repository is already in it; such a row can't be picked.
#[derive(Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub private: bool,
    pub taken: Option<Field>,
    pub typed: bool,
}

/// The dropdown for `field`: the listed repositories not in it yet that match `query`, an exact match first and
/// then most recently pushed first, as listed; then the typed name when it is `owner/name` and listed nowhere.
pub fn rows(listed: &[AccessibleRepo], query: &str, lists: &Lists, field: Field) -> Vec<Row> {
    let q = query.trim();
    let (mine, other) = (&lists[field as usize], &lists[field.other() as usize]);
    let mut out: Vec<Row> = listed
        .iter()
        .map(|r| r.full_name.as_str())
        .zip(listed.iter().map(|r| r.private))
        .filter(|(name, _)| !has(mine, name) && fuzzy(name, q))
        .take(MENU_ROWS)
        .map(|(name, private)| Row {
            name: name.to_string(),
            private,
            taken: has(other, name).then_some(field.other()),
            typed: false,
        })
        .collect();
    out.sort_by_key(|r| !r.name.eq_ignore_ascii_case(q));
    if valid(q) && !has(mine, q) && !listed.iter().any(|r| r.full_name.as_str().eq_ignore_ascii_case(q)) {
        out.push(Row {
            name: q.to_string(),
            private: false,
            taken: has(other, q).then_some(field.other()),
            typed: true,
        });
    }
    out
}

/// The next row ↓ (or ↑) lands on from `from`, skipping rows that can't be picked; `None` when none can.
pub fn step(rows: &[Row], from: Option<usize>, down: bool) -> Option<usize> {
    let open: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.taken.is_none()).map(|(i, _)| i).collect();
    match from.and_then(|f| open.iter().position(|&i| i == f)) {
        None if down => open.first().copied(),
        None => open.last().copied(),
        Some(p) if down => open.get(p + 1).or(open.last()).copied(),
        Some(p) => open.get(p.saturating_sub(1)).copied(),
    }
}

/// What ⏎ adds: the highlighted row, else the typed name. `Ok(None)` adds nothing; `Err` says why it can't.
pub fn pick(
    rows: &[Row],
    highlight: Option<usize>,
    query: &str,
    lists: &Lists,
    field: Field,
) -> Result<Option<String>, String> {
    if let Some(r) = highlight.and_then(|i| rows.get(i)).filter(|r| r.taken.is_none()) {
        return Ok(Some(r.name.clone()));
    }
    let q = query.trim();
    let other = field.other();
    if q.is_empty() || has(&lists[field as usize], q) {
        Ok(None)
    } else if !valid(q) {
        Err(format!("“{q}” is not owner/name."))
    } else if has(&lists[other as usize], q) {
        Err(format!("{q} is in {} already.", other.word()))
    } else {
        Ok(Some(q.to_string()))
    }
}

impl Shell {
    /// Makes the fields the first time Settings draws, opens a staged dropdown, and sends lists waiting to be saved.
    pub fn repo_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for f in Field::BOTH {
            if self.repos.combo(f).input.is_none() {
                let input =
                    cx.new(|cx| InputState::new(window, cx).placeholder("Search repositories, or type owner/name"));
                let events = cx.subscribe_in(&input, window, move |s, _, ev: &InputEvent, window, cx| {
                    s.combo_event(f, ev, window, cx)
                });
                let c = self.repos.combo_mut(f);
                c.input = Some(input);
                c._events = Some(events);
            }
        }
        if let Some((f, query)) = self.repos.staged.take()
            && let Some(input) = self.repos.combo(f).input.clone()
        {
            input.update(cx, |i, cx| i.set_value(query, window, cx));
            self.combo_open(f, cx);
        }
        self.flush_repos(cx);
    }

    fn daemon_lists(&self) -> Option<Lists> {
        let a = self.model.agent.settings.as_ref()?.auto_review.as_ref()?;
        let names = |l: &[pr_review_tui::protocol::Text]| l.iter().map(|n| n.to_string()).collect();
        Some([names(&a.repos.include), names(&a.repos.exclude)])
    }

    /// The lists as the reader sees them: waiting to be sent, sent, else the daemon's.
    fn repo_lists(&self) -> Option<Lists> {
        self.repos.want.clone().or_else(|| self.repos.sent.clone()).or_else(|| self.daemon_lists())
    }

    fn flush_repos(&mut self, cx: &mut Context<Self>) {
        let a = &self.model.agent;
        if matches!(a.auto_save, SaveState::Sending { .. }) {
            return;
        }
        if let Some([include, exclude]) = self.repos.want.take() {
            self.repos.sent = Some([include.clone(), exclude.clone()]);
            let repos = RepoParams { include: Some(include), exclude: Some(exclude) };
            return self.send(Msg::SetAutoReview(AutoReviewParams { repos: Some(repos), ..Default::default() }), cx);
        }
        let reading_back = matches!(a.auto_save, SaveState::Saved(_)) && a.request.is_some();
        if !reading_back {
            self.repos.sent = None;
        }
    }

    fn edit_repos(&mut self, f: Field, change: impl FnOnce(&mut Vec<String>), cx: &mut Context<Self>) {
        let Some(mut lists) = self.repo_lists() else { return };
        change(&mut lists[f as usize]);
        self.repos.want = Some(lists);
        self.flush_repos(cx);
        cx.notify();
    }

    fn combo_rows(&self, f: Field, cx: &App) -> Vec<Row> {
        let listed = self.model.agent.repos.as_ref().map(|r| r.repos.as_slice()).unwrap_or_default();
        let query = self.repos.combo(f).input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
        rows(listed, &query, &self.repo_lists().unwrap_or_default(), f)
    }

    fn combo_open(&mut self, f: Field, cx: &mut Context<Self>) {
        let first = step(&self.combo_rows(f, cx), None, true);
        let c = self.repos.combo_mut(f);
        c.open = true;
        c.highlight = first;
        c.problem = None;
        cx.notify();
    }

    fn combo_event(&mut self, f: Field, ev: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            InputEvent::Focus | InputEvent::Change => self.combo_open(f, cx),
            InputEvent::Blur => {
                self.repos.combo_mut(f).open = false;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => self.combo_enter(f, window, cx),
        }
    }

    fn combo_enter(&mut self, f: Field, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.combo_rows(f, cx);
        let c = self.repos.combo(f);
        let query = c.input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
        let highlight = c.highlight.filter(|_| c.open);
        match pick(&rows, highlight, &query, &self.repo_lists().unwrap_or_default(), f) {
            Ok(Some(name)) => self.add_repo(f, name, window, cx),
            Ok(None) => {}
            Err(why) => {
                self.repos.combo_mut(f).problem = Some(why);
                cx.notify();
            }
        }
    }

    fn add_repo(&mut self, f: Field, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_repos(f, |l| l.push(name), cx);
        if let Some(input) = self.repos.combo(f).input.clone() {
            input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.combo_open(f, cx);
    }

    fn combo_move(&mut self, f: Field, down: bool, cx: &mut Context<Self>) {
        let rows = self.combo_rows(f, cx);
        let c = self.repos.combo_mut(f);
        c.highlight = if c.open { step(&rows, c.highlight, down) } else { step(&rows, None, true) };
        c.open = true;
        cx.stop_propagation();
        cx.notify();
    }

    fn combo_escape(&mut self, f: Field, cx: &mut Context<Self>) {
        let c = self.repos.combo_mut(f);
        if c.open {
            c.open = false;
            cx.stop_propagation();
            cx.notify();
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

/// The last poll as one line: when, and what it found and started, or why it failed, or that the day's cap stops it.
pub fn last_poll(t: &Colors, a: &AutoReview) -> Option<(IconName, Hsla, String)> {
    let Some(p) = &a.last_poll else {
        return a.enabled.then(|| {
            (IconName::Clock, t.muted, "Not checked yet: the reviewer checks shortly after it starts.".to_string())
        });
    };
    let at = crate::kit::clock(p.at.as_str());
    let cap = a.daily_cap.unwrap_or_default();
    let today = p.started_today.unwrap_or_default();
    Some(match &p.error {
        Some(why) => (
            IconName::CircleAlert,
            t.danger,
            crate::kit::sentence(&format!("last check at {at} failed: {why}; it tries again later")),
        ),
        None if p.cap_reached => {
            (IconName::CircleSlash, t.warn, format!("Daily cap reached ({today} of {cap} today). Resumes tomorrow."))
        }
        None => {
            let day = match p.started_today {
                Some(n) if cap > 0 => format!(", {n} of {cap} today"),
                Some(n) => format!(", {n} today"),
                None => String::new(),
            };
            (
                IconName::CircleCheck,
                t.ok,
                format!("Last checked at {at}: found {}, started {}, skipped {}{day}.", p.found, p.started, p.skipped),
            )
        }
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

    let per_day = match a.daily_cap {
        Some(n) if n > 0 => format!(" and {n} a day"),
        _ => String::new(),
    };
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
                         your review, and starts each commit it hasn't reviewed, at most 5 at a time{per_day}. \
                         Archived PRs stay archived. Each review costs model usage.",
                        a.interval_minutes
                    ),
                    t.muted,
                )),
        );

    let choose = |id: String, name: &'static str, on: bool, words: String, params: AutoReviewParams| {
        (
            id,
            name,
            "",
            on,
            words,
            crate::ui::click(cx, move |s, cx| {
                if !matches!(s.model.agent.auto_save, SaveState::Sending { .. })
                    && write_block(&s.model, "settings.set").is_none()
                {
                    s.send(Msg::SetAutoReview(params.clone()), cx)
                }
            }),
            None,
        )
    };
    let every = segmented(
        t,
        INTERVALS.map(|(n, name)| {
            let params = AutoReviewParams { interval_minutes: Some(n), ..Default::default() };
            choose(format!("auto-every-{n}"), name, a.interval_minutes == n, format!("Check every {n} minutes"), params)
        }),
    );
    let caps = a.daily_cap.map(|cap| {
        segmented(
            t,
            CAPS.map(|(n, name)| {
                let words = if n == 0 { "Start any number a day".into() } else { format!("Start at most {n} a day") };
                let params = AutoReviewParams { daily_cap: Some(n), ..Default::default() };
                choose(format!("auto-cap-{n}"), name, cap == n, words, params)
            }),
        )
    });
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

    let lists = s.repo_lists().unwrap_or_default();
    let status = match &m.agent.auto_save {
        SaveState::Sending { .. } => Some((t.muted, "Saving…".to_string())),
        SaveState::Refused(why) => Some((t.danger, crate::kit::sentence(&format!("not saved: {why}")))),
        SaveState::TimedOut => Some((t.danger, "The reviewer did not answer in time. Try again.".to_string())),
        SaveState::Idle | SaveState::Saved(_) => {
            block.map(|why| (t.muted, crate::kit::sentence(&format!("can't change it here: {why}"))))
        }
    };

    let body =
        div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .p(space::S4)
            .child(head)
            .children(
                publishing(t, settings.publish_mode.as_ref().map(|p| p.as_str())).map(|(hue, w)| line(w.into(), hue)),
            )
            .child(row().child(label("Check every")).child(every))
            .children(caps.map(|c| row().child(label("Daily cap")).child(c)))
            .child(row().child(label("Team requests")).child(teams))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(space::S3)
                    .child(label("Only these repositories").pt(px(6.)))
                    .child(combo(s, t, Field::Only, &lists, block.is_none(), cx)),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(space::S3)
                    .child(label("Never these repositories").pt(px(6.)))
                    .child(combo(s, t, Field::Never, &lists, block.is_none(), cx)),
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

/// One list as removable chips over a search field, and under it, while the field has the keyboard, the dropdown.
fn combo(s: &Shell, t: &Colors, f: Field, lists: &Lists, can: bool, cx: &Context<Shell>) -> Div {
    let c = s.repos.combo(f);
    let mine = &lists[f as usize];
    let empty = match f {
        Field::Only => "Every repository",
        Field::Never => "None",
    };
    let chips = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(space::S1)
        .when(mine.is_empty(), |d| d.child(div().py(px(3.)).text_size(text::SMALL).text_color(t.muted).child(empty)))
        .children(mine.iter().map(|name| repo_chip(t, f, name, can, cx)));
    let field = div()
        .font_family(text::MONO)
        .text_size(text::CODE)
        .children(c.input.as_ref().map(|i| Input::new(i).disabled(!can)))
        .capture_action(cx.listener(move |s, _: &MoveDown, _, cx| s.combo_move(f, true, cx)))
        .capture_action(cx.listener(move |s, _: &MoveUp, _, cx| s.combo_move(f, false, cx)))
        .capture_action(cx.listener(move |s, _: &Escape, _, cx| s.combo_escape(f, cx)));
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(chips)
        .child(field)
        .when(c.open && can, |d| d.child(menu(s, t, f, cx)))
        .children(c.problem.clone().map(|why| div().text_size(text::SMALL).text_color(t.danger).child(why)))
}

fn repo_chip(t: &Colors, f: Field, name: &str, can: bool, cx: &Context<Shell>) -> Div {
    let gone = name.to_string();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1)
        .h(px(24.))
        .pl(space::S2)
        .pr(if can { px(2.) } else { space::S2 })
        .rounded(radius::SM)
        .bg(t.selected)
        .border_1()
        .border_color(t.border_strong)
        .font_family(text::MONO)
        .text_size(text::CODE)
        .text_color(t.text)
        .child(name.to_string())
        .when(can, |d| {
            d.child(crate::kit::icon_button(
                t,
                SharedString::from(format!("auto-{}-remove-{name}", f.id())),
                IconName::Close,
                &format!("Remove {name}"),
                px(20.),
                crate::ui::on(cx, move |s, cx| {
                    let gone = gone.clone();
                    s.edit_repos(f, move |l| l.retain(|x| *x != gone), cx)
                }),
            ))
        })
}

/// The dropdown: the account's repositories that match, or why there are none to offer.
fn menu(s: &Shell, t: &Colors, f: Field, cx: &Context<Shell>) -> Div {
    let rows = s.combo_rows(f, cx);
    let highlight = s.repos.combo(f).highlight;
    let note =
        |words: String| div().px(space::S3).py(space::S2).text_size(text::SMALL).text_color(t.muted).child(words);
    let query = s.repos.combo(f).input.as_ref().map(|i| i.read(cx).value().trim().to_string()).unwrap_or_default();
    let state = match &s.model.agent.repos {
        None => Some("Loading your repositories…".to_string()),
        Some(r) => match &r.error {
            Some(why) => Some(crate::kit::sentence(&format!(
                "couldn't list your repositories: {why}. Type owner/name and press ⏎ to add one"
            ))),
            None if rows.is_empty() && !query.is_empty() => Some(format!("No repository matches “{query}”.")),
            None if rows.is_empty() => Some("Every repository you can reach is in a list.".to_string()),
            None => None,
        },
    };
    let list = div()
        .flex()
        .flex_col()
        .py(space::S1)
        .children(rows.iter().enumerate().map(|(i, r)| menu_row(t, f, r, highlight == Some(i), cx)));
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(radius::SM)
        .border_1()
        .border_color(t.border_strong)
        .bg(t.raised)
        .shadow(vec![BoxShadow {
            color: t.shadow.alpha(0.24),
            offset: point(px(0.), px(6.)),
            blur_radius: px(18.),
            spread_radius: px(0.),
            inset: false,
        }])
        .overflow_hidden()
        .children(state.map(note))
        .when(!rows.is_empty(), |d| {
            d.child(
                div()
                    .id(SharedString::from(format!("auto-{}-menu", f.id())))
                    .max_h(MENU_HEIGHT)
                    .child(list)
                    .overflow_y_scrollbar(),
            )
        })
}

fn menu_row(t: &Colors, f: Field, r: &Row, lit: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let name = r.name.clone();
    let open = r.taken.is_none();
    let ink = if open { t.text } else { t.disabled };
    div()
        .id(SharedString::from(format!("auto-{}-row-{}", f.id(), r.name)))
        .flex()
        .items_center()
        .gap(space::S2)
        .px(space::S3)
        .h(px(30.))
        .when(lit, |d| d.bg(t.selected))
        .when(open && !lit, |d| d.hover(|h| h.bg(t.hover)))
        .when(open, |d| {
            d.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |s, _: &MouseDownEvent, window, cx| {
                    // Down, not click: the field keeps the keyboard, so the dropdown stays open for the next pick.
                    cx.stop_propagation();
                    s.add_repo(f, name.clone(), window, cx);
                }),
            )
        })
        .child(div().w(px(14.)).flex_none().children(if r.typed {
            Some(icon(IconName::Plus, px(13.), t.muted))
        } else if r.private {
            Some(icon(IconName::Lock, px(12.), t.muted))
        } else {
            None
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(text::MONO)
                .text_size(text::CODE)
                .text_color(ink)
                .child(if r.typed { format!("Add {}", r.name) } else { r.name.clone() }),
        )
        .children(r.taken.map(|other| {
            div().flex_none().text_size(text::SMALL).text_color(t.disabled).child(format!("in {}", other.word()))
        }))
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
    use super::{Field, Lists, Row, pick, rows, step, valid};
    use pr_review_tui::protocol::{AccessibleRepo, Text};

    fn listed(names: &[&str]) -> Vec<AccessibleRepo> {
        names
            .iter()
            .map(|n| AccessibleRepo {
                full_name: Text::new(n),
                private: n.starts_with("acme/"),
                archived: false,
                pushed_at: None,
            })
            .collect()
    }

    fn lists(only: &[&str], never: &[&str]) -> Lists {
        [only.iter().map(|s| s.to_string()).collect(), never.iter().map(|s| s.to_string()).collect()]
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn repositories_are_owner_name() {
        assert!(valid("acme/shop") && valid("x.y/z-1") && valid("a_b/c"));
        assert!(!valid("acme") && !valid("acme/shop/extra") && !valid("acme/ü") && !valid("/shop") && !valid("acme/"));
    }

    #[test]
    fn the_dropdown_matches_fuzzily_keeps_the_push_order_and_leaves_out_what_the_list_has() {
        let repos = listed(&["acme/shop", "octo/dotfiles", "acme/api", "northwind/shipping", "acme/shop-legacy"]);
        let none = lists(&[], &[]);
        assert_eq!(names(&rows(&repos, "", &none, Field::Only)).len(), 5);
        assert_eq!(
            names(&rows(&repos, "shp", &none, Field::Only)),
            ["acme/shop", "northwind/shipping", "acme/shop-legacy"]
        );
        assert_eq!(names(&rows(&repos, " ACME/ ", &none, Field::Only)), ["acme/shop", "acme/api", "acme/shop-legacy"]);
        assert_eq!(names(&rows(&repos, "acme/shop-legacy", &none, Field::Only)), ["acme/shop-legacy"]);
        assert_eq!(names(&rows(&repos, "shop", &lists(&["acme/shop"], &[]), Field::Only)), ["acme/shop-legacy"]);
        assert_eq!(rows(&repos, "shop", &lists(&["ACME/SHOP"], &[]), Field::Only)[0].name, "acme/shop-legacy");
        assert!(rows(&repos, "", &none, Field::Only)[0].private);
    }

    #[test]
    fn an_exact_match_comes_first() {
        let repos = listed(&["acme/api-gateway", "acme/api"]);
        assert_eq!(names(&rows(&repos, "acme/api", &lists(&[], &[]), Field::Only)), ["acme/api", "acme/api-gateway"]);
    }

    #[test]
    fn a_repository_in_the_other_list_shows_there_and_cant_be_picked() {
        let repos = listed(&["acme/shop", "acme/api", "acme/web"]);
        let r = rows(&repos, "acme", &lists(&["acme/api"], &[]), Field::Never);
        assert_eq!(r[1], Row { name: "acme/api".into(), private: true, taken: Some(Field::Only), typed: false });
        assert_eq!(
            (step(&r, None, true), step(&r, Some(0), true), step(&r, Some(2), false)),
            (Some(0), Some(2), Some(0))
        );
        assert_eq!(
            pick(&r, Some(1), "acme", &lists(&["acme/api"], &[]), Field::Never),
            Err("“acme” is not owner/name.".into()),
            "a taken row is never what ⏎ adds"
        );
        assert_eq!(
            pick(&[], None, "acme/api", &lists(&["acme/api"], &[]), Field::Never),
            Err("acme/api is in Only already.".into())
        );
    }

    #[test]
    fn arrows_walk_the_rows_and_stop_at_the_ends() {
        let r = rows(&listed(&["a/one", "a/two", "a/three"]), "", &lists(&[], &[]), Field::Only);
        assert_eq!(step(&r, None, true), Some(0));
        assert_eq!(step(&r, None, false), Some(2));
        assert_eq!(step(&r, Some(0), true), Some(1));
        assert_eq!(step(&r, Some(2), true), Some(2));
        assert_eq!(step(&r, Some(0), false), Some(0));
        assert_eq!(step(&[], None, true), None);
    }

    #[test]
    fn a_typed_name_nobody_lists_is_offered_last_and_enter_adds_it_when_it_is_owner_name() {
        let repos = listed(&["acme/shop"]);
        let none = lists(&[], &[]);
        let r = rows(&repos, "other/lib", &none, Field::Only);
        assert_eq!(r, [Row { name: "other/lib".into(), private: false, taken: None, typed: true }]);
        assert_eq!(pick(&r, step(&r, None, true), "other/lib", &none, Field::Only), Ok(Some("other/lib".into())));
        assert!(rows(&repos, "acme/shop", &none, Field::Only).iter().all(|r| !r.typed));
        assert_eq!(pick(&[], None, "not a repo", &none, Field::Only), Err("“not a repo” is not owner/name.".into()));
        assert_eq!(pick(&[], None, "  ", &none, Field::Only), Ok(None));
        assert_eq!(pick(&[], None, "other/lib", &lists(&["Other/Lib"], &[]), Field::Only), Ok(None));
        assert_eq!(pick(&[], None, "other/lib", &none, Field::Only), Ok(Some("other/lib".into())), "the list failed");
    }

    #[test]
    fn the_highlighted_row_wins_over_the_typed_text() {
        let repos = listed(&["acme/shop", "acme/api"]);
        let r = rows(&repos, "acme", &lists(&[], &[]), Field::Only);
        assert_eq!(pick(&r, Some(1), "acme", &lists(&[], &[]), Field::Only), Ok(Some("acme/api".into())));
    }
}
