//! Where the app lives once its window closes: a menu bar item that opens it again or quits it, an optional Dock
//! icon, and a login item that starts it in the background.

use std::path::{Path, PathBuf};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What the menu bar item asks of the window, beyond showing it.
pub enum Ask {
    CheckForUpdates,
}

/// The LaunchAgent that opens the app at login. Its plist being there is the whole setting.
const LOGIN_LABEL: &str = "io.github.diego-arbio-dev.pr-review.app";

/// Started by the login item: the window opens hidden, so login puts the app in the menu bar and nothing in front.
pub const BACKGROUND_FLAG: &str = "--background";

fn login_plist() -> PathBuf {
    crate::reviewer::home().join(format!("Library/LaunchAgents/{LOGIN_LABEL}.plist"))
}

pub fn opens_at_login() -> bool {
    login_plist().exists()
}

/// Writes or removes the login item. launchd reads the folder at the next login, so nothing is loaded now.
pub fn set_open_at_login(on: bool) -> Result<(), String> {
    let path = login_plist();
    let at = |e: std::io::Error| format!("{}: {e}", path.display());
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(at(e)),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe().map_err(|e| format!("can't find the app: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(at)?;
    }
    std::fs::write(&path, login_agent(&exe)).map_err(at)
}

/// The plist: `open` on the app bundle when the binary is in one, so the app starts as macOS starts apps, else the
/// binary itself.
fn login_agent(exe: &Path) -> String {
    let args: Vec<String> = match bundle(exe) {
        Some(app) => {
            vec![
                "/usr/bin/open".into(),
                "-a".into(),
                app.display().to_string(),
                "--args".into(),
                BACKGROUND_FLAG.into(),
            ]
        }
        None => vec![exe.display().to_string(), BACKGROUND_FLAG.into()],
    };
    let args: String = args.iter().map(|a| format!("\n        <string>{}</string>", xml(a))).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LOGIN_LABEL}</string>
    <key>ProgramArguments</key>
    <array>{args}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
</dict>
</plist>
"#
    )
}

/// The `.app` the binary runs from, when it runs from one.
pub fn bundle(exe: &Path) -> Option<&Path> {
    exe.ancestors().nth(3).filter(|b| b.extension().is_some_and(|e| e == "app"))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// What the menu bar item shows of the reviewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glance {
    /// The eye without its pupil, dimmed: the reviewer doesn't answer.
    pub asleep: bool,
    /// The pupil's offset from the middle, in viewBox units: it glances side to side while reviews run.
    pub pupil: i8,
    /// The Needs you count beside the eye.
    pub count: Option<usize>,
    /// The state in words, for the tooltip and the menu's first line.
    pub line: String,
}

/// How long the pupil rests at each place while reviews run.
const GLANCE_MS: u64 = 400;

/// `link` is the title bar's word for the reviewer; `now_ms` is `None` when the reader asked for less motion.
pub fn glance(link: &str, live: bool, running: usize, waiting: usize, now_ms: Option<u64>) -> Glance {
    if !live {
        return Glance { asleep: true, pupil: 0, count: None, line: format!("Reviewer: {link}") };
    }
    let mut line = vec!["Reviewer live".to_string()];
    if running > 0 {
        line.push(format!("{running} reviewing"));
    }
    match waiting {
        0 => {}
        1 => line.push("1 needs you".into()),
        n => line.push(format!("{n} need you")),
    }
    let pupil = match now_ms {
        Some(t) if running > 0 => [0, 2, 0, -2][(t / GLANCE_MS % 4) as usize],
        _ => 0,
    };
    Glance { asleep: false, pupil, count: (waiting > 0).then_some(waiting), line: line.join(" · ") }
}

/// Shows the window's state on the menu bar item.
pub fn reflect(s: &crate::ui::Shell, cx: &gpui_kit::App) {
    let m = &s.model;
    let live = matches!(m.link, pr_review_tui::model::Link::Live);
    let waiting = crate::queue::decisions(&crate::queue::cards(m));
    let (_, link) = crate::ui::link_word(s, &crate::ui::colors(cx));
    let now = (!s.still).then_some(m.now_ms);
    paint(&glance(&link, live, crate::queue::running(m).len(), waiting, now));
}

#[cfg(not(target_os = "macos"))]
fn paint(_: &Glance) {}

/// Brings the window back: out of a hidden app, off the Dock if minimized, and in front.
#[cfg(target_os = "macos")]
fn show(mtm: objc2::MainThreadMarker) {
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    app.unhide(None);
    for w in app.windows() {
        if w.isMiniaturized() {
            w.deminiaturize(None);
        }
        w.makeKeyAndOrderFront(None);
    }
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

#[cfg(target_os = "macos")]
mod tray {
    use std::cell::RefCell;
    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSApplication, NSCellImagePosition, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
        NSVariableStatusItemLength,
    };
    use objc2_foundation::{NSData, NSSize, NSString};

    use super::{Ask, Glance, UnboundedReceiver, UnboundedSender, unbounded_channel};

    /// The item, its menu's status line, and what they show now, so an unchanged state costs nothing.
    struct Shown {
        item: Retained<NSStatusItem>,
        line: Retained<NSMenuItem>,
        glance: Option<Glance>,
    }

    thread_local! {
        static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    }

    /// Where the menu's asks go: `install` hands the window the other end.
    static ASKS: OnceLock<UnboundedSender<Ask>> = OnceLock::new();

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "PRReviewMenuBarTarget"]
        struct Target;

        impl Target {
            #[unsafe(method(open:))]
            fn open(&self, _: Option<&AnyObject>) {
                super::show(self.mtm());
            }

            #[unsafe(method(checkForUpdates:))]
            fn check_for_updates(&self, _: Option<&AnyObject>) {
                super::show(self.mtm());
                if let Some(asks) = ASKS.get() {
                    let _ = asks.send(Ask::CheckForUpdates);
                }
            }

            #[unsafe(method(quit:))]
            fn quit(&self, _: Option<&AnyObject>) {
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
        }
    );

    /// The app's mark as a template image, so macOS tints it for a light or dark menu bar: the eye, and its pupil
    /// `pupil` units from the middle when it has one.
    fn mark(pupil: Option<i8>) -> Option<Retained<NSImage>> {
        let eye = include_str!("../assets/mark.svg").trim_end().trim_end_matches("</svg>");
        let pupil = pupil.map_or(String::new(), |dx| {
            format!(r##"<circle cx="{}" cy="12" r="2.2" fill="#000" stroke="none"/>"##, 12 + i32::from(dx))
        });
        let data = NSData::with_bytes(format!("{eye}{pupil}</svg>").as_bytes());
        let image = NSImage::initWithData(NSImage::alloc(), &data)?;
        image.setSize(NSSize::new(18., 18.));
        image.setTemplate(true);
        Some(image)
    }

    /// Puts the item in the menu bar for the life of the process, and returns what its menu asks of the window.
    pub fn install() -> UnboundedReceiver<Ask> {
        let (tx, rx) = unbounded_channel();
        let Some(mtm) = MainThreadMarker::new() else { return rx };
        let _ = ASKS.set(tx);
        let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            match mark(Some(0)) {
                Some(image) => button.setImage(Some(&image)),
                None => button.setTitle(&NSString::from_str("PR")),
            }
            button.setImagePosition(NSCellImagePosition::ImageLeft);
            button.setToolTip(Some(&NSString::from_str(crate::ui::APP_NAME)));
        }
        let menu = NSMenu::new(mtm);
        let entry = |title: String, action, key: &str| {
            let title = NSString::from_str(&title);
            let key = NSString::from_str(key);
            let entry =
                unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &title, action, &key) };
            unsafe { entry.setTarget(Some(&target)) };
            entry
        };
        // No action, so the menu shows it greyed: it reports, it doesn't do.
        let line = entry("Connecting…".into(), None, "");
        menu.addItem(&line);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&entry(format!("Open {}", crate::ui::APP_NAME), Some(sel!(open:)), ""));
        menu.addItem(&entry("Check for Updates…".into(), Some(sel!(checkForUpdates:)), ""));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&entry(format!("Quit {}", crate::ui::APP_NAME), Some(sel!(quit:)), "q"));
        item.setMenu(Some(&menu));
        SHOWN.with(|s| *s.borrow_mut() = Some(Shown { item, line, glance: None }));
        // A menu item holds its target weakly: it lives as long as the process, as the item does in SHOWN.
        std::mem::forget(target);
        rx
    }

    pub fn paint(glance: &Glance) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        SHOWN.with(|s| {
            let mut s = s.borrow_mut();
            let Some(s) = s.as_mut().filter(|s| s.glance.as_ref() != Some(glance)) else { return };
            if let Some(button) = s.item.button(mtm) {
                if let Some(image) = mark((!glance.asleep).then_some(glance.pupil)) {
                    button.setImage(Some(&image));
                }
                button.setAppearsDisabled(glance.asleep);
                let count = glance.count.map_or(String::new(), |n| format!(" {n}"));
                button.setTitle(&NSString::from_str(&count));
                button.setToolTip(Some(&NSString::from_str(&format!("{}: {}", crate::ui::APP_NAME, glance.line))));
            }
            s.line.setTitle(&NSString::from_str(&glance.line));
            s.glance = Some(glance.clone());
        });
    }
}

#[cfg(target_os = "macos")]
pub use tray::install;
#[cfg(target_os = "macos")]
use tray::paint;

#[cfg(not(target_os = "macos"))]
pub fn install() -> UnboundedReceiver<Ask> {
    unbounded_channel::<Ask>().1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eye_closes_offline_glances_while_reviewing_and_counts_what_needs_you() {
        let off = glance("Stopped by you", false, 2, 3, Some(0));
        assert_eq!(off, Glance { asleep: true, pupil: 0, count: None, line: "Reviewer: Stopped by you".into() });

        let idle = glance("Live", true, 0, 0, Some(400));
        assert_eq!(idle, Glance { asleep: false, pupil: 0, count: None, line: "Reviewer live".into() });

        let busy = |t| glance("Live", true, 2, 1, t);
        assert_eq!(busy(Some(0)).line, "Reviewer live · 2 reviewing · 1 needs you");
        assert_eq!(busy(Some(0)).count, Some(1));
        let looks: Vec<i8> = [0, 400, 800, 1200, 1600].map(|t| busy(Some(t)).pupil).to_vec();
        assert_eq!(looks, [0, 2, 0, -2, 0]);
        assert_eq!(busy(None).pupil, 0, "reduce motion keeps the pupil still");
        assert_eq!(glance("Live", true, 0, 3, Some(0)).line, "Reviewer live · 3 need you");
    }

    #[test]
    fn the_login_item_opens_the_bundle_in_the_background() {
        let plist =
            login_agent(Path::new("/Users/me/Applications/PR Reviewer & Co.app/Contents/MacOS/pr-review-desktop"));
        assert!(plist.contains(
            "<string>/usr/bin/open</string>\n        <string>-a</string>\n        \
             <string>/Users/me/Applications/PR Reviewer &amp; Co.app</string>\n        <string>--args</string>\n        \
             <string>--background</string>"
        ));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));

        let bare = login_agent(Path::new("/src/desktop/target/release/pr-review-desktop"));
        assert!(bare.contains("<array>\n        <string>/src/desktop/target/release/pr-review-desktop</string>"));
        assert!(!bare.contains("/usr/bin/open"));
    }
}
