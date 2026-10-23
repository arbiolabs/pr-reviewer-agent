//! The sidebar's footer: the GitHub account the daemon publishes as, its avatar, and the connection under it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui_kit::assets::IconName;
use gpui_kit::*;
use pr_review_tui::keymap::Action;
use pr_review_tui::model::{Identity, Model};
use pr_review_tui::protocol::{Account, AccountState};

use crate::kit::{focusable, icon, tip};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::{Shell, link_word};

/// A cached avatar older than this is fetched again; until the fetch lands the old one shows.
const TTL: Duration = Duration::from_secs(24 * 60 * 60);
const SIZE: Pixels = px(24.);

/// The account `Model::identity` says the daemon publishes as, or the words for why there is none.
fn account(m: &Model) -> Result<&Account, &'static str> {
    match m.identity() {
        Some(Identity::Selected(a) | Identity::GhActive(a)) => Ok(a),
        Some(Identity::Env) => Err("Env token"),
        Some(Identity::Nobody) => Err("No GitHub account"),
        None => Err("No account reported"),
    }
}

/// The avatar's file under `dir`, for a github.com account whose login GitHub could have issued: letters, digits
/// and inner hyphens. Anything else from the daemon never becomes a path or a URL.
fn cache_path(dir: &Path, a: &Account) -> Option<PathBuf> {
    let login = a.login.as_str();
    let shaped = !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    (a.host.as_str() == "github.com" && shaped).then(|| dir.join(format!("{login}.png")))
}

fn cache_dir() -> PathBuf {
    crate::reviewer::home().join("Library/Caches/pr-review/avatars")
}

fn fresh(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified).is_ok_and(|age| age < TTL)
}

/// The letter a monogram shows: the login's first, upper-cased.
fn monogram(login: &str) -> String {
    login.chars().next().map_or_else(|| "?".into(), |c| c.to_uppercase().collect())
}

/// Fetches `login`'s 64 px avatar (48 device pixels at 24 pt on Retina, so it stays crisp) into `dest` with the
/// system's curl, through a temporary file, so a reader never sees half an image.
fn fetch(login: &str, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
    let part = dest.with_extension("part");
    let url = format!("https://avatars.githubusercontent.com/{login}?s=64");
    let ok = std::process::Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "15", "-o"])
        .arg(&part)
        .arg(&url)
        .status()?
        .success();
    if !ok {
        let _ = std::fs::remove_file(&part);
        return Err(std::io::Error::other(format!("curl could not fetch {url}")));
    }
    std::fs::rename(&part, dest)
}

/// What the footer's avatar shows: the image file once it is on disk, for the login it was asked for.
#[derive(Default)]
pub struct Avatar {
    login: Option<String>,
    image: Option<PathBuf>,
}

/// What the avatar does with the account's cache file, last modified at `modified` if it exists: the file to show
/// now, and whether to fetch it. A snapshot shows the monogram and never touches the network or the cache.
fn plan(
    snapshot: bool,
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    now: SystemTime,
) -> (Option<PathBuf>, bool) {
    match path {
        Some(p) if !snapshot => {
            let stale = !modified.is_some_and(|t| fresh(t, now));
            (modified.map(|_| p), stale)
        }
        _ => (None, false),
    }
}

impl Shell {
    /// Points the avatar at the publishing account: a cached file shows at once, and a missing or stale one is
    /// fetched in the background.
    pub fn sync_avatar(&mut self, cx: &mut Context<Self>) {
        let a = account(&self.model).ok();
        let login = a.map(|a| a.login.to_string());
        if self.avatar.login == login {
            return;
        }
        let path = a.and_then(|a| cache_path(&cache_dir(), a));
        let modified = path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let (image, stale) = plan(self.snapshot, path.clone(), modified, SystemTime::now());
        self.avatar = Avatar { login: login.clone(), image };
        let (Some(path), Some(login), true) = (path, login, stale) else { return };
        cx.spawn(async move |this, cx| {
            let (dest, who) = (path.clone(), login.clone());
            let got = cx.background_executor().spawn(async move { fetch(&who, &dest) }).await;
            this.update(cx, |s, cx| {
                if got.is_ok() && s.avatar.login.as_deref() == Some(login.as_str()) {
                    s.avatar.image = Some(path);
                    cx.notify();
                }
            })
        })
        .detach();
    }
}

/// The monogram: the login's first letter in the accent over its wash, the avatar's size, so nothing moves when
/// the image lands or fails.
fn letter(t: &Colors, login: &str) -> Div {
    div()
        .size(SIZE)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::PILL)
        .bg(t.accent.alpha(t.tint))
        .border_1()
        .border_color(t.accent.alpha(t.tint * 2.))
        .text_size(text::SMALL)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.accent)
        .child(monogram(login))
}

/// No account to show: a person glyph in the same circle, in the neutral ink.
fn nobody(t: &Colors) -> Div {
    div()
        .size(SIZE)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::PILL)
        .border_1()
        .border_color(t.border_strong)
        .child(icon(IconName::User, px(14.), t.muted))
}

fn state_word(s: AccountState) -> Option<&'static str> {
    match s {
        AccountState::Ok => None,
        AccountState::Error => Some("Error"),
        AccountState::Unknown => Some("Unknown"),
    }
}

/// Avatar and `login` on one line, the connection under it, and Read-only beside it when the daemon runs no
/// commands. One tab stop: a click or ⏎ opens Settings at the GitHub accounts.
pub fn footer(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Stateful<Div> {
    let m = &s.model;
    let (hue, word) = link_word(s, t);
    let endpoint = match m.diag.endpoint.split_once(':').map_or("", |(k, _)| k) {
        "unix" => "socket",
        other => other,
    }
    .to_string();
    let read_only = crate::queue::read_only(m);
    let who = account(m);
    let face = match (who, &s.avatar.image) {
        (Ok(a), Some(path)) => {
            let (t, login) = (*t, a.login.to_string());
            let (t2, login2) = (t, login.clone());
            img(path.clone())
                .size(SIZE)
                .flex_none()
                .rounded(radius::PILL)
                .with_loading(move || letter(&t, &login).into_any_element())
                .with_fallback(move || letter(&t2, &login2).into_any_element())
                .into_any_element()
        }
        (Ok(a), None) => letter(t, a.login.as_str()).into_any_element(),
        (Err(_), _) => nobody(t).into_any_element(),
    };
    let (name, name_hue) = match who {
        Ok(a) => (a.login.to_string(), t.text),
        Err(words) => (words.to_string(), t.muted),
    };
    let warn = who.ok().and_then(|a| state_word(a.state));
    let mut status = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(16.))
        .text_size(text::CAPTION)
        .child(div().size(px(7.)).flex_none().rounded(radius::PILL).bg(hue))
        .child(div().flex_none().text_color(t.muted).child(word.clone()))
        .child(div().flex_none().text_color(t.faint).child("·"))
        .child(div().min_w_0().truncate().text_color(t.faint).child(endpoint));
    if read_only {
        status = status
            .child(div().flex_none().text_color(t.faint).child("·"))
            .child(div().flex_none().text_color(t.warn).child("Read-only"));
    }
    let row = div().flex().items_center().gap(space::S2).h(px(52.)).px(space::S4).child(face).child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(18.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(text::BODY)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(name_hue)
                            .child(name),
                    )
                    .children(warn.map(|w| div().flex_none().text_size(text::CAPTION).text_color(t.warn).child(w))),
            )
            .child(status),
    );
    let signed_in = match who {
        Ok(a) => format!("Signed in to GitHub as {}", a.login),
        Err(words) => words.to_string(),
    };
    let label = format!(
        "{signed_in}{}, connection {}{}",
        warn.map(|w| format!(" ({})", w.to_lowercase())).unwrap_or_default(),
        word.to_lowercase(),
        if read_only { ", read-only" } else { "" },
    );
    let detail = match who {
        Ok(a) if a.scopes.is_empty() => format!("{}\nNo scopes reported", a.id),
        Ok(a) => format!("{}\nScopes: {}", a.id, a.scopes.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(", ")),
        Err(words) => words.to_string(),
    };
    let tooltip = format!("{detail}\n{}\nClick for Settings", m.diag.endpoint);
    let footer = focusable(
        "account",
        row,
        t,
        radius::SM,
        Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::Accounts, cx))),
    )
    .role(Role::Button)
    .aria_label(label);
    tip(div().id("account-footer").border_t_1().border_color(t.border).child(footer), tooltip)
}

#[cfg(test)]
mod tests {
    use super::{TTL, account, cache_path, fresh, monogram, plan};
    use pr_review_tui::model::{Model, Screen};
    use pr_review_tui::protocol::Account;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn acct(id: &str, host: &str, login: &str, state: &str, gh_active: bool, selected: bool) -> Account {
        serde_json::from_value(serde_json::json!({
            "id": id, "host": host, "login": login, "state": state,
            "ghActive": gh_active, "selected": selected, "scopes": ["repo"],
        }))
        .unwrap()
    }

    fn model(accounts: Option<Vec<Account>>) -> Model {
        let mut m = Model::new("unix:/tmp/s.sock", Default::default(), Screen::Inbox, None);
        m.domain.accounts = accounts;
        m
    }

    fn name(m: &Model) -> String {
        account(m).map_or_else(|w| w.to_string(), |a| format!("@{}", a.login))
    }

    #[test]
    fn the_footer_names_the_account_the_daemon_publishes_as() {
        let active = acct("github.com/active", "github.com", "active", "ok", true, false);
        let chosen = acct("github.com/chosen", "github.com", "chosen", "ok", false, true);
        let env = acct("env/GITHUB_TOKEN", "env", "GITHUB_TOKEN", "ok", false, false);
        let ghe = acct("ghe.acme.io/boss", "ghe.acme.io", "boss", "ok", true, false);
        assert_eq!(name(&model(Some(vec![active.clone(), chosen.clone()]))), "@chosen");
        assert_eq!(name(&model(Some(vec![active.clone()]))), "@active");
        assert_eq!(name(&model(Some(vec![chosen.clone(), env.clone()]))), "Env token", "the env token wins");
        assert_eq!(name(&model(Some(vec![ghe]))), "No GitHub account", "gh active counts on github.com only");
        assert_eq!(name(&model(Some(vec![]))), "No GitHub account");
        assert_eq!(name(&model(None)), "No account reported");
    }

    #[test]
    fn a_monogram_is_the_logins_first_letter() {
        assert_eq!(monogram("diego-arbio-dev"), "D");
        assert_eq!(monogram("ñu"), "Ñ");
        assert_eq!(monogram(""), "?");
    }

    #[test]
    fn only_a_github_login_becomes_a_cache_path() {
        let dir = Path::new("/c/avatars");
        let ok = acct("github.com/diego-arbio-dev", "github.com", "diego-arbio-dev", "ok", true, false);
        assert_eq!(cache_path(dir, &ok), Some(PathBuf::from("/c/avatars/diego-arbio-dev.png")));
        for (host, login) in [("ghe.acme.io", "boss"), ("github.com", "../x"), ("github.com", "-a"), ("github.com", "")]
        {
            assert_eq!(cache_path(dir, &acct("x", host, login, "ok", true, false)), None, "{host} {login}");
        }
    }

    #[test]
    fn a_cached_avatar_is_fresh_for_a_day() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        assert!(fresh(now - Duration::from_secs(60), now));
        assert!(!fresh(now - TTL, now));
        assert!(!fresh(now + Duration::from_secs(5), now), "a file from the future is refetched");
    }

    #[test]
    fn a_snapshot_never_fetches_or_reads_the_cache() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        let p = || Some(PathBuf::from("/c/a.png"));
        let hour_ago = Some(now - Duration::from_secs(3600));
        let week_ago = Some(now - Duration::from_secs(7 * 86_400));
        assert_eq!(plan(true, p(), hour_ago, now), (None, false));
        assert_eq!(plan(true, p(), None, now), (None, false));
        assert_eq!(plan(false, p(), hour_ago, now), (p(), false), "fresh: show it, no fetch");
        assert_eq!(plan(false, p(), week_ago, now), (p(), true), "stale: show the old one and refetch");
        assert_eq!(plan(false, p(), None, now), (None, true), "missing: the monogram until it lands");
        assert_eq!(plan(false, None, None, now), (None, false), "no github.com login: the monogram");
    }
}
